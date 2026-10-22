//! Capture → OCR → translate → overlay, in one place so the CLI and the
//! preview window run exactly the same code.

use std::fmt;
use std::path::{Path, PathBuf};

use cosmic_screenshot_ocr::{self as ocr, TextBlock};
use cosmic_screenshot_overlay::{self as overlay, Canvas, FontSet, Options, Rect, TranslatedBlock};
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
    match languages_override {
        Some(languages) if !languages.trim().is_empty() => {
            config.languages = languages.trim().to_string();
        }
        _ => {
            if std::env::var(ocr::LANGUAGES_VARIABLE)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                config.languages = lang::all_ocr_languages();
            }
        }
    }
    config
}

pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
    pub target: Lang,
    /// Explicit source language; `None` means detect it from the capture.
    pub source: Option<Lang>,
    pub ocr: ocr::Config,
    pub options: Options,
}

impl Job {
    pub fn new(input: impl Into<PathBuf>, target: Lang) -> Self {
        let input = input.into();
        let output = default_output(&input);
        Self {
            input,
            output,
            target,
            source: None,
            ocr: ocr_config(None),
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
}

/// The result of translating blocks into an existing canvas.
#[derive(Debug)]
pub struct Applied {
    pub report: overlay::Report,
    pub detected_source: Option<Lang>,
}

pub fn run(job: &Job, translator: &dyn Translator) -> Result<Outcome, PipelineError> {
    let mut canvas = Canvas::open(&job.input).map_err(PipelineError::Overlay)?;
    let blocks = ocr::recognize(&job.input, &job.ocr).map_err(PipelineError::Ocr)?;
    let applied = apply(
        &mut canvas,
        &blocks,
        job.target,
        job.source,
        translator,
        &job.options,
    )?;
    canvas.save(&job.output).map_err(PipelineError::Overlay)?;
    Ok(Outcome {
        output: job.output.clone(),
        blocks: blocks.len(),
        report: applied.report,
        detected_source: applied.detected_source,
    })
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
