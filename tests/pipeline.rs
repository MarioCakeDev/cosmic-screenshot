//! The pipeline in isolation: OCR blocks in, painted canvas out, with the fake
//! translator standing in for the real engine.

use cosmic_screenshot_engine as pipeline;
use cosmic_screenshot_engine::Error as PipelineError;
use cosmic_screenshot_ocr::{Bbox, TextBlock};
use cosmic_screenshot_overlay::{Canvas, Options};
use cosmic_screenshot_translation::Lang;
use cosmic_screenshot_translation::testing::{FakeTranslator, UnconfiguredTranslator};

fn blocks() -> Vec<TextBlock> {
    vec![
        TextBlock {
            bbox: Bbox {
                x: 40,
                y: 40,
                width: 160,
                height: 22,
            },
            text: "Hello".to_string(),
            confidence: 95.0,
        },
        TextBlock {
            bbox: Bbox {
                x: 40,
                y: 80,
                width: 200,
                height: 22,
            },
            text: "World".to_string(),
            confidence: 92.0,
        },
    ]
}

fn capture() -> Canvas {
    let mut canvas = Canvas::filled(300, 140, [250, 250, 250, 255]);
    for block in blocks() {
        for y in block.bbox.y..block.bbox.y + block.bbox.height {
            for x in block.bbox.x..block.bbox.x + block.bbox.width {
                canvas.set_pixel(x, y, [20, 20, 20, 255]);
            }
        }
    }
    canvas
}

#[test]
fn one_batched_call_covers_every_block() {
    let mut canvas = capture();
    let translator = FakeTranslator::new("[de] ");
    let applied = pipeline::apply(
        &mut canvas,
        &blocks(),
        Lang::parse("de").unwrap(),
        None,
        &translator,
        &Options::default(),
    )
    .expect("pipeline succeeds");

    assert_eq!(applied.report.drawn, 2);
    assert_eq!(applied.report.covered_only, 0);
    assert_eq!(applied.detected_source.map(Lang::code), Some("en"));

    let calls = translator.calls();
    assert_eq!(
        calls.len(),
        1,
        "OCR blocks must be batched into one request"
    );
    assert_eq!(calls[0].texts, vec!["Hello", "World"]);
    assert_eq!(calls[0].target.code(), "de");
    assert_eq!(calls[0].source, None, "source language is auto-detected");
}

#[test]
fn empty_translation_still_covers_the_original() {
    let mut canvas = capture();
    let translator = FakeTranslator::echoing();
    // A provider that returns empty strings: the old text must still go away.
    let blocks: Vec<TextBlock> = blocks()
        .into_iter()
        .map(|block| TextBlock {
            text: String::new(),
            ..block
        })
        .collect();
    let applied = pipeline::apply(
        &mut canvas,
        &blocks,
        Lang::parse("de").unwrap(),
        None,
        &translator,
        &Options::default(),
    )
    .expect("pipeline succeeds");
    assert_eq!(applied.report.covered_only, 2);
    assert_eq!(canvas.pixel(60, 50), [250, 250, 250, 255]);
}

#[test]
fn no_text_is_a_clear_error() {
    let mut canvas = capture();
    let error = pipeline::apply(
        &mut canvas,
        &[],
        Lang::parse("de").unwrap(),
        None,
        &FakeTranslator::echoing(),
        &Options::default(),
    )
    .unwrap_err();
    assert!(matches!(error, PipelineError::NoText));
    assert!(error.to_string().contains("no text found"));
}

#[test]
fn an_unconfigured_engine_does_not_touch_the_canvas() {
    let mut canvas = capture();
    let before = canvas.pixel(60, 50);
    let error = pipeline::apply(
        &mut canvas,
        &blocks(),
        Lang::parse("de").unwrap(),
        None,
        &UnconfiguredTranslator,
        &Options::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "translation engine not configured: set DEEPL_API_KEY"
    );
    assert_eq!(
        canvas.pixel(60, 50),
        before,
        "nothing may be painted on failure"
    );
}

#[test]
fn an_explicit_source_reaches_the_engine() {
    let mut canvas = capture();
    let translator = FakeTranslator::new("[de] ");
    pipeline::apply(
        &mut canvas,
        &blocks(),
        Lang::parse("de").unwrap(),
        Some(Lang::parse("en").unwrap()),
        &translator,
        &Options::default(),
    )
    .expect("pipeline succeeds");
    let calls = translator.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source.map(Lang::code), Some("en"));
}

#[test]
fn an_assumed_source_is_flagged_for_the_ui() {
    let mut canvas = capture();
    let translator = FakeTranslator::new("[de] ").assuming_the_default_source();
    let applied = pipeline::apply(
        &mut canvas,
        &blocks(),
        Lang::parse("de").unwrap(),
        None,
        &translator,
        &Options::default(),
    )
    .expect("pipeline succeeds");
    assert!(applied.source_assumed);
    assert_eq!(applied.detected_source.map(Lang::code), Some("en"));
}

#[test]
fn detects_the_target_language_from_its_code() {
    let mut canvas = capture();
    let mut translator = FakeTranslator::new("[fr] ");
    translator = translator.with_detected_language("de");
    let applied = pipeline::apply(
        &mut canvas,
        &blocks(),
        Lang::parse("fr").unwrap(),
        None,
        &translator,
        &Options::default(),
    )
    .unwrap();
    assert_eq!(applied.detected_source.map(Lang::code), Some("de"));
}
