//! Capture → OCR → translate → overlay, in one place so the CLI and the
//! preview window run exactly the same code.

use std::fmt;
use std::path::{Path, PathBuf};

use cosmic_screenshot_ocr::{self as ocr, TextBlock};
use cosmic_screenshot_overlay::{self as overlay, Canvas, FontSet, Options, Rect, TranslatedBlock};
use cosmic_screenshot_translation::{
    self as translation, Detected, Lang, TranslateError, Translator,
};

use crate::lang;

/// Environment variable that selects the engine. The local NLLB sidecar is the
/// default; DeepL is kept for paid DeepL accounts.
pub const PROVIDER_VARIABLE: &str = "TRANSLATE_PROVIDER";

/// Build the configured translator, failing with one clear line when the
/// engine cannot be used (sidecar down, missing model, missing key).
pub fn translator_from_env() -> Result<Box<dyn Translator>, TranslateError> {
    let requested = std::env::var(PROVIDER_VARIABLE)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| translation::Provider::default().id().to_string());
    let provider = translation::Provider::from_id(&requested).ok_or(TranslateError::BadConfig {
        variable: PROVIDER_VARIABLE,
        value: requested,
        hint: "use 'local-nmt' (the default) or 'deepl'",
    })?;
    match provider {
        translation::Provider::LocalNmt => Ok(Box::new(translation::LocalTranslator::from_env()?)),
        translation::Provider::DeepL => Ok(Box::new(translation::DeepLTranslator::from_env()?)),
        other => Err(TranslateError::Unsupported { provider: other }),
    }
}

/// OCR configuration: the language table's packs, unless the environment or
/// the command line narrows them.
pub fn ocr_config(languages_override: Option<String>) -> ocr::Config {
    let mut config = ocr::Config::from_env();
    match ocr_languages_override(languages_override.as_deref()) {
        Some(languages) => config.languages = languages,
        None => config.languages = lang::all_ocr_languages(),
    }
    config
}

/// The `-l` packs the user chose explicitly, from the flag or the environment.
///
/// This is also the signal that turns the two-pass OCR off: a chosen list means
/// the user knows the source language, and re-reading with a detected pack
/// would override that choice.
pub fn ocr_languages_override(flag: Option<&str>) -> Option<String> {
    let from_env = std::env::var(ocr::LANGUAGES_VARIABLE).ok();
    explicit_languages(flag, from_env.as_deref())
}

fn explicit_languages(flag: Option<&str>, from_env: Option<&str>) -> Option<String> {
    flag.or(from_env)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The packs for a confirmation OCR pass, or `None` when there is no point.
///
/// All packs at once is the only way to read text whose language is unknown,
/// but it is worse at the language it eventually turns out to be: a 24-pack
/// Tesseract reads `für` as `fur` and `Rücksendungen` as `Ricksendungen`. Once
/// detection has named the language, re-read the capture with that pack alone.
/// A low-confidence fallback is not a detection, and a language with no row in
/// [`lang::LANGS`] has no pack to name.
pub fn confirmation_languages(detected: Detected) -> Option<&'static str> {
    if detected.assumed {
        return None;
    }
    lang::find(detected.lang.code()).map(|info| info.tesseract)
}

pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
    pub target: Lang,
    /// Explicit source language; `None` means detect it from the capture.
    pub source: Option<Lang>,
    pub ocr: ocr::Config,
    /// Re-read the capture with the detected language's pack alone. Off when
    /// the user chose the OCR packs (`--ocr-langs`, the environment).
    pub two_pass_ocr: bool,
    pub options: Options,
}

impl Job {
    pub fn new(input: impl Into<PathBuf>, target: Lang) -> Self {
        // The environment can choose the packs too; a chosen list is the user's
        // decision, and the preview window must honour it just as the headless
        // path does. Resolve it once, then build from the resolved value.
        let from_env = std::env::var(ocr::LANGUAGES_VARIABLE).ok();
        Self::with_ocr_override(input, target, explicit_languages(None, from_env.as_deref()))
    }

    /// `languages` is the already-resolved override: `None` means the full pack
    /// list, `Some` means the user chose exactly those packs. Taking it as a
    /// parameter keeps the decision testable without mutating the environment.
    fn with_ocr_override(
        input: impl Into<PathBuf>,
        target: Lang,
        languages: Option<String>,
    ) -> Self {
        let input = input.into();
        let output = default_output(&input);
        let ocr = match &languages {
            Some(chosen) => ocr::Config::default().with_languages(chosen.clone()),
            None => ocr::Config::default().with_languages(lang::all_ocr_languages()),
        };
        Self {
            input,
            output,
            target,
            source: None,
            ocr,
            two_pass_ocr: languages.is_none(),
            options: Options::default(),
        }
    }
}

/// `shot.png` → `shot.translated.png`.
pub fn default_output(input: &Path) -> PathBuf {
    input.with_extension("translated.png")
}

#[derive(Debug)]
pub struct Outcome {
    pub output: PathBuf,
    pub blocks: usize,
    pub report: overlay::Report,
    pub detected_source: Option<Lang>,
    /// The source language came from a low-confidence fallback, not detection.
    pub source_assumed: bool,
}

impl Outcome {
    /// "German", "English (assumed)", or "the source language" — one phrase for
    /// the notification and the preview status.
    pub fn source_label(&self) -> String {
        match (self.detected_source, self.source_assumed) {
            (Some(lang), true) => format!("{} (assumed)", lang.english_name()),
            (Some(lang), false) => lang.english_name().to_string(),
            (None, _) => "the source language".to_string(),
        }
    }
}

/// The result of translating blocks into an existing canvas.
#[derive(Debug)]
pub struct Applied {
    pub report: overlay::Report,
    pub detected_source: Option<Lang>,
    pub source_assumed: bool,
}

pub fn run(job: &Job, translator: &dyn Translator) -> Result<Outcome, PipelineError> {
    let mut canvas = Canvas::open(&job.input).map_err(PipelineError::Overlay)?;
    let (blocks, source) = recognize_blocks(job, translator, &|config| {
        ocr::recognize(&job.input, config)
    })?;
    let applied = apply(
        &mut canvas,
        &blocks,
        job.target,
        source,
        translator,
        &job.options,
    )?;
    canvas.save(&job.output).map_err(PipelineError::Overlay)?;
    Ok(Outcome {
        output: job.output.clone(),
        blocks: blocks.len(),
        report: applied.report,
        detected_source: applied.detected_source,
        source_assumed: applied.source_assumed,
    })
}

/// OCR the capture, with the second pass when the source has to be detected.
///
/// The recognizer is a parameter so the decision can be tested without
/// Tesseract; production passes [`ocr::recognize`] bound to `job.input`.
fn recognize_blocks(
    job: &Job,
    translator: &dyn Translator,
    recognize: &dyn Fn(&ocr::Config) -> Result<Vec<TextBlock>, ocr::OcrError>,
) -> Result<(Vec<TextBlock>, Option<Lang>), PipelineError> {
    let blocks = recognize(&job.ocr).map_err(PipelineError::Ocr)?;
    if !job.two_pass_ocr || job.source.is_some() || blocks.is_empty() {
        return Ok((blocks, job.source));
    }

    let texts: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
    let detected = translator
        .detect_source(&texts)
        .map_err(PipelineError::Translation)?;
    let Some(languages) = detected.and_then(confirmation_languages) else {
        return Ok((blocks, None));
    };

    let narrowed = job.ocr.clone().with_languages(languages);
    // A narrower pack can find nothing, or be missing from the install; the
    // first pass already suffices then. A mixed-script capture can come back
    // partial: the other script's lines keep their original pixels, which is
    // safe but leaves them untranslated.
    match recognize(&narrowed) {
        Ok(second) if !second.is_empty() => Ok((second, detected.map(|detected| detected.lang))),
        _ => Ok((blocks, None)),
    }
}

/// Translate already-recognised blocks and paint them onto `canvas`.
///
/// Separate from [`run`] so the pipeline can be exercised without Tesseract.
pub fn apply(
    canvas: &mut Canvas,
    blocks: &[TextBlock],
    target: Lang,
    source: Option<Lang>,
    translator: &dyn Translator,
    options: &Options,
) -> Result<Applied, PipelineError> {
    if blocks.is_empty() {
        return Err(PipelineError::NoText);
    }
    let texts: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
    let batch = translator
        .translate(&texts, target, source)
        .map_err(PipelineError::Translation)?;
    if batch.items.len() != blocks.len() {
        return Err(PipelineError::Translation(TranslateError::CountMismatch {
            requested: blocks.len(),
            received: batch.items.len(),
        }));
    }

    let detected_source = batch.detected_source();
    let source_assumed = batch.source_assumed;
    let translated: Vec<TranslatedBlock> = blocks
        .iter()
        .zip(batch.items)
        .map(|(block, item)| TranslatedBlock {
            rect: rect_of(block),
            text: item.text,
        })
        .collect();

    let script = lang::find(target.code())
        .map(|info| info.script)
        .unwrap_or(overlay::Script::Latin);
    let fonts = FontSet::system(script).map_err(PipelineError::Overlay)?;
    let report = overlay::overlay(canvas, &translated, &fonts, options);

    Ok(Applied {
        report,
        detected_source,
        source_assumed,
    })
}

fn rect_of(block: &TextBlock) -> Rect {
    Rect::new(
        block.bbox.x as f32,
        block.bbox.y as f32,
        block.bbox.width as f32,
        block.bbox.height as f32,
    )
}

#[derive(Debug)]
pub enum PipelineError {
    Ocr(ocr::OcrError),
    Translation(TranslateError),
    Overlay(overlay::OverlayError),
    NoText,
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::Ocr(error) => write!(f, "{error}"),
            PipelineError::Translation(error) => write!(f, "{error}"),
            PipelineError::Overlay(error) => write!(f, "{error}"),
            PipelineError::NoText => {
                f.write_str("no text found in the captured area — nothing to translate")
            }
        }
    }
}

impl std::error::Error for PipelineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_screenshot_translation::testing::FakeTranslator;

    fn outcome(detected: Option<Lang>, assumed: bool) -> Outcome {
        Outcome {
            output: PathBuf::from("/tmp/shot.translated.png"),
            blocks: 1,
            report: overlay::Report {
                blocks: 1,
                drawn: 1,
                ..overlay::Report::default()
            },
            detected_source: detected,
            source_assumed: assumed,
        }
    }

    fn detected(code: &str, assumed: bool) -> Detected {
        Detected {
            lang: Lang::parse(code).expect("test language"),
            assumed,
        }
    }

    fn line(text: &str) -> TextBlock {
        TextBlock {
            bbox: ocr::Bbox {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            text: text.to_string(),
            confidence: 95.0,
        }
    }

    /// Run `recognize_blocks` with a fake recognizer, recording each `-l` list.
    fn passes(
        job: &Job,
        translator: &FakeTranslator,
        second: Result<Vec<TextBlock>, ocr::OcrError>,
    ) -> (Vec<TextBlock>, Option<Lang>, Vec<String>) {
        let configs = std::cell::RefCell::new(Vec::new());
        let second = std::cell::RefCell::new(Some(second));
        let result = recognize_blocks(job, translator, &|config| {
            let first = configs.borrow().is_empty();
            configs.borrow_mut().push(config.languages.clone());
            if first {
                Ok(vec![line("für")])
            } else {
                second
                    .borrow_mut()
                    .take()
                    .expect("the second pass runs at most once")
            }
        });
        let (blocks, source) = result.expect("recognizing succeeds");
        (blocks, source, configs.into_inner())
    }

    #[test]
    fn the_second_pass_re_reads_with_the_detected_pack_alone() {
        let job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        let all = job.ocr.languages.clone();
        let translator = FakeTranslator::new("").with_detection("de");

        let (blocks, source, configs) = passes(&job, &translator, Ok(vec![line("für")]));

        assert_eq!(configs, vec![all, "deu".to_string()]);
        assert_eq!(source.map(Lang::code), Some("de"));
        assert_eq!(blocks[0].text, "für");
    }

    #[test]
    fn an_assumed_detection_leaves_the_single_pass_alone() {
        let job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        let translator = FakeTranslator::new("").assuming_the_source_on_detection("en");

        let (_, source, configs) = passes(&job, &translator, Ok(vec![line("x")]));

        assert_eq!(configs.len(), 1, "an assumed language must not re-read");
        assert_eq!(source, None, "the sidecar detects again when translating");
    }

    #[test]
    fn an_explicit_source_skips_detection_entirely() {
        let mut job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        job.source = Some(Lang::parse("de").unwrap());
        let translator = FakeTranslator::new("").with_detection("ja");

        let (_, source, configs) = passes(&job, &translator, Ok(vec![line("x")]));

        assert_eq!(configs.len(), 1);
        assert_eq!(source.map(Lang::code), Some("de"));
    }

    #[test]
    fn chosen_ocr_packs_disable_the_second_pass() {
        let mut job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        job.ocr.languages = "eng+deu".to_string();
        job.two_pass_ocr = false;
        let translator = FakeTranslator::new("").with_detection("ja");

        let (_, source, configs) = passes(&job, &translator, Ok(vec![line("x")]));

        assert_eq!(configs, vec!["eng+deu".to_string()]);
        assert_eq!(source, None);
    }

    #[test]
    fn a_failed_or_empty_second_pass_keeps_the_first_reading() {
        let job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        let translator = FakeTranslator::new("").with_detection("de");

        let failed = passes(
            &job,
            &translator,
            Err(ocr::OcrError::Spawn("no deu".into())),
        );
        assert_eq!(failed.0[0].text, "für");
        assert_eq!(failed.1, None, "the first pass has no detection yet");

        let empty = passes(&job, &translator, Ok(Vec::new()));
        assert_eq!(empty.0[0].text, "für");
        assert_eq!(empty.1, None);
    }

    #[test]
    fn an_empty_capture_costs_no_detection() {
        let job = Job::new("/tmp/shot.png", Lang::parse("en").unwrap());
        let translator = FakeTranslator::new("").with_detection("de");
        let configs = std::cell::RefCell::new(Vec::new());

        let result = recognize_blocks(&job, &translator, &|config| {
            configs.borrow_mut().push(config.languages.clone());
            Ok(Vec::new())
        })
        .expect("recognizing succeeds");

        assert!(result.0.is_empty());
        assert_eq!(configs.into_inner().len(), 1, "nothing to detect");
    }

    #[test]
    fn the_confirmation_pass_uses_the_detected_pack_alone() {
        assert_eq!(confirmation_languages(detected("de", false)), Some("deu"));
        assert_eq!(confirmation_languages(detected("ja", false)), Some("jpn"));
        assert_eq!(
            confirmation_languages(detected("zh", false)),
            Some("chi_sim+chi_tra"),
            "the Chinese row carries both packs"
        );
    }

    #[test]
    fn an_assumed_language_never_narrows_the_ocr_packs() {
        assert_eq!(confirmation_languages(detected("en", true)), None);
    }

    #[test]
    fn the_flag_outranks_the_environment_and_blank_values_do_not_count() {
        assert_eq!(
            explicit_languages(Some(" eng+deu "), Some("fra")).as_deref(),
            Some("eng+deu")
        );
        assert_eq!(
            explicit_languages(None, Some("fra")).as_deref(),
            Some("fra")
        );
        assert_eq!(explicit_languages(None, None), None);
        assert_eq!(explicit_languages(Some("  "), None), None);
    }

    #[test]
    fn a_job_without_chosen_packs_reads_twice_with_every_pack() {
        let job = Job::with_ocr_override("/tmp/shot.png", Lang::parse("en").unwrap(), None);
        assert_eq!(job.ocr.languages, lang::all_ocr_languages());
        assert!(job.two_pass_ocr);
    }

    #[test]
    fn a_job_with_chosen_packs_reads_once_with_exactly_those_packs() {
        let job = Job::with_ocr_override(
            "/tmp/shot.png",
            Lang::parse("en").unwrap(),
            Some("eng+deu".to_string()),
        );
        assert_eq!(job.ocr.languages, "eng+deu");
        assert!(
            !job.two_pass_ocr,
            "the preview must not override packs the user chose"
        );
    }

    #[test]
    fn the_source_label_distinguishes_assumed_from_detected() {
        assert_eq!(
            outcome(Some(Lang::parse("de").unwrap()), false).source_label(),
            "German"
        );
        assert_eq!(
            outcome(Some(Lang::parse("en").unwrap()), true).source_label(),
            "English (assumed)",
            "a low-confidence fallback must not look like a detection"
        );
        assert_eq!(outcome(None, false).source_label(), "the source language");
    }
}
