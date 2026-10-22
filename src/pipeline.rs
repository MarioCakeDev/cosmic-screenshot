//! Files in, translated file out.
//!
//! The OCR → detect → translate → overlay work lives in
//! [`cosmic_screenshot_engine`], which the portal screenshot dialog uses too.
//! What remains here is the CLI's own job description: an input path, an output
//! path, and the engine configuration read from the environment.

use std::path::{Path, PathBuf};

use cosmic_screenshot_engine::{self as engine, Applied, Error, Request};
use cosmic_screenshot_ocr as ocr;
use cosmic_screenshot_overlay::{Canvas, Options};
use cosmic_screenshot_translation::{self as translation, Lang, TranslateError, Translator};

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
        // decision. Resolve it once, then build from the resolved value.
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

    fn request(&self) -> Request {
        Request {
            target: self.target,
            source: self.source,
            ocr: self.ocr.clone(),
            two_pass_ocr: self.two_pass_ocr,
            options: self.options,
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
    pub report: cosmic_screenshot_overlay::Report,
    pub detected_source: Option<Lang>,
    /// The source language came from a low-confidence fallback, not detection.
    pub source_assumed: bool,
}

impl Outcome {
    /// "German", "English (assumed)", or "the source language" — one phrase for
    /// the notification.
    pub fn source_label(&self) -> String {
        engine::source_label(self.detected_source, self.source_assumed)
    }
}

impl From<Applied> for Outcome {
    fn from(applied: Applied) -> Self {
        Self {
            output: PathBuf::new(),
            blocks: applied.blocks,
            report: applied.report,
            detected_source: applied.detected_source,
            source_assumed: applied.source_assumed,
        }
    }
}

pub fn run(job: &Job, translator: &dyn Translator) -> Result<Outcome, Error> {
    let mut canvas = Canvas::open(&job.input).map_err(Error::Overlay)?;
    let applied = engine::translate_canvas(&mut canvas, &job.request(), translator, &|config| {
        ocr::recognize(&job.input, config)
    })?;
    canvas.save(&job.output).map_err(Error::Overlay)?;
    Ok(Outcome {
        output: job.output.clone(),
        ..applied.into()
    })
}

/// A one-line error for the CLI, with the same wording for every failure.
pub type PipelineError = Error;

#[cfg(test)]
mod tests {
    use super::*;

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
            "the CLI must not override packs the user chose"
        );
    }

    #[test]
    fn the_default_output_sits_beside_the_input() {
        assert_eq!(
            default_output(Path::new("/tmp/shot.png")),
            PathBuf::from("/tmp/shot.translated.png")
        );
    }
}
