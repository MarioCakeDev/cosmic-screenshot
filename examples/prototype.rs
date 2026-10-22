//! Prototype harness: OCR → translate → overlay on a static image.
//!
//! Draws a synthetic "screenshot" with ab_glyph, reads it with the real
//! Tesseract (so the TSV layer is exercised end to end), translates through a
//! dictionary fake — no engine needed — and writes before/after PNGs.
//!
//!   cargo run --example prototype -- /tmp/translate-demo
//!
//! Requires `tesseract-ocr` and `tesseract-ocr-eng` installed.

use std::path::PathBuf;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use cosmic_screenshot_ocr as ocr;
use cosmic_screenshot_overlay::{Canvas, Options};
use cosmic_screenshot_translation::Lang;

const HEADING: &str = "Good morning";
const LINE_ONE: &str = "Where is the train station?";
const LINE_TWO: &str = "The library closes at eight.";

struct Dictionary;

impl Dictionary {
    fn translate(&self, text: &str) -> String {
        match text
            .trim_end_matches(['.', '?', '!'])
            .to_ascii_lowercase()
            .as_str()
        {
            "good morning" => "Guten Morgen".to_string(),
            "where is the train station" => "Wo ist der Bahnhof?".to_string(),
            "the library closes at eight" => "Die Bibliothek schließt um acht.".to_string(),
            _ => format!("[{text}]"),
        }
    }
}

fn main() {
    let directory: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/translate-demo".to_string())
        .into();
    std::fs::create_dir_all(&directory).expect("create output directory");

    let font = FontArc::try_from_vec(
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("DejaVu Sans is installed (fonts-dejavu-core)"),
    )
    .expect("valid font");

    let mut canvas = Canvas::filled(960, 300, [246, 246, 244, 255]);
    // A card, so the overlay has real "paper" to sample.
    for y in 20..280 {
        for x in 20..940 {
            canvas.set_pixel(x, y, [255, 255, 255, 255]);
        }
    }

    draw(&mut canvas, &font, HEADING, 60, 90, 44.0, [24, 24, 30, 255]);
    draw(
        &mut canvas,
        &font,
        LINE_ONE,
        60,
        170,
        30.0,
        [40, 40, 48, 255],
    );
    draw(
        &mut canvas,
        &font,
        LINE_TWO,
        60,
        230,
        30.0,
        [40, 40, 48, 255],
    );

    let before = directory.join("before.png");
    canvas.save(&before).expect("write before.png");

    let blocks = ocr::recognize(&before, &ocr::Config::default().with_languages("eng"))
        .expect("tesseract must be installed for this example");
    println!("OCR found {} line(s):", blocks.len());
    for block in &blocks {
        println!("  {:?} {:?}", block.bbox, block.text);
    }

    let dictionary = Dictionary;
    let texts: Vec<String> = blocks
        .iter()
        .map(|b| dictionary.translate(&b.text))
        .collect();

    let mut canvas = Canvas::open(&before).expect("reopen before.png");
    let translated: Vec<cosmic_screenshot_overlay::TranslatedBlock> = blocks
        .iter()
        .zip(texts)
        .map(|(block, text)| cosmic_screenshot_overlay::TranslatedBlock {
            rect: cosmic_screenshot_overlay::Rect::new(
                block.bbox.x as f32,
                block.bbox.y as f32,
                block.bbox.width as f32,
                block.bbox.height as f32,
            ),
            text,
        })
        .collect();
    let fonts =
        cosmic_screenshot_overlay::FontSet::system(cosmic_screenshot_overlay::Script::Latin)
            .expect("a Latin font is installed");
    let report =
        cosmic_screenshot_overlay::overlay(&mut canvas, &translated, &fonts, &Options::default());

    let after = directory.join("after.png");
    canvas.save(&after).expect("write after.png");

    println!(
        "painted {} block(s), detected source {:?}",
        report.drawn,
        Lang::parse("en").map(|l| l.english_name())
    );
    println!("before: {}", before.display());
    println!("after:  {}", after.display());
}

fn draw(
    canvas: &mut Canvas,
    font: &FontArc,
    text: &str,
    x: u32,
    baseline: u32,
    size: f32,
    color: [u8; 4],
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut pen = x as f32;
    for character in text.chars() {
        let glyph = scaled.glyph_id(character);
        if let Some(outline) = font.outline_glyph(scaled.scaled_glyph(character)) {
            let bounds = outline.px_bounds();
            let origin_x = pen + bounds.min.x;
            let origin_y = baseline as f32 + bounds.min.y;
            outline.draw(|dx, dy, coverage| {
                let px = (origin_x + dx as f32).round() as u32;
                let py = (origin_y + dy as f32).round() as u32;
                if px >= canvas.width || py >= canvas.height {
                    return;
                }
                let destination = canvas.pixel(px, py);
                let alpha = coverage.clamp(0.0, 1.0);
                let mut blended = [0u8; 4];
                for channel in 0..3 {
                    blended[channel] = (color[channel] as f32 * alpha
                        + destination[channel] as f32 * (1.0 - alpha))
                        .round() as u8;
                }
                blended[3] = 255;
                canvas.set_pixel(px, py, blended);
            });
        }
        pen += scaled.h_advance(glyph);
    }
}
