# Design note — Lens-style translation of a captured region

Fork: <https://github.com/MarioCakeDev/cosmic-screenshot> (upstream history kept,
nothing pushed to `pop-os`).

## What the tool does

1. Capture a region through the XDG desktop portal — the stock `cosmic-screenshot`
   selection UI, unchanged.
2. OCR the captured image and keep each recognised **line** with its box.
3. Translate every line in **one** DeepL request (batched, source language
   auto-detected by DeepL itself).
4. Cover each original line with the sampled paper colour and draw the translation
   into the same box: lens-style, in place, not a side panel.
5. Show the result in a preview window with a **Translate** button and a
   **target-language picker**, and save the composited PNG — the saved image
   contains the translated overlay, not the original text.

`--translate <language>` runs steps 1–4 headlessly and prints the output path.

## Where the button lives (and why)

`cosmic-screenshot` upstream is a ~150-line CLI: it asks the portal for a
screenshot and saves the file. **The area-selection UI is not in this repository**
— it belongs to `xdg-desktop-portal-cosmic`, which the whole desktop uses.

Adding the button to the portal would mean replacing a system-wide service (and
would break the stock tool and other portal consumers). Instead the fork adds its
own preview window on the captured region, and that window is where the
Translate button and the language picker live. The portal still does what it is
good at (region selection); everything from OCR onwards is ours.

The preview window uses `iced` with the `tiny-skia` software renderer: no GPU
dependency, no libcosmic build (which would dominate a `target/` directory on the
91 %-full maripop root filesystem), and it renders identically on any session.

## Translation: vendor-neutral, DeepL by default

`crates/translation` exposes the only interface the pipeline knows:

```rust
trait Translator {
    fn provider(&self) -> Provider;
    fn translate(&self, texts: &[&str], target: Lang, source: Option<Lang>)
        -> Result<Batch, TranslateError>;
}
```

`Provider` is an enum over `DeepL | GoogleCloud | Azure | LocalNmt`. DeepL is
implemented; the other variants are expressible in configuration
(`TRANSLATE_PROVIDER`) and fail with one clear line rather than a panic. Adding a
provider is one `impl Translator`; nothing in OCR or overlay changes.

### DeepL specifics

| Concern | Decision |
|---|---|
| Endpoint | `https://api-free.deepl.com/v2/translate` by default; `DEEPL_HOST=paid` → `https://api.deepl.com`; any full URL also accepted (proxies/tests) |
| Why configurable | a free-tier key against the paid host (or vice versa) is a 403 with a misleading body; the host is explicit rather than guessed |
| Key | `DEEPL_API_KEY`, read at runtime. Never logged, never committed, redacted in `Debug`, never in a fixture |
| Not configured | `translation engine not configured: set DEEPL_API_KEY` — one line, exit code 1, and the capture is not taken |
| Source language | omitted from the request; DeepL returns `detected_source_language` per item, which is surfaced in the status/notification |
| Batching | all OCR lines in one request; chunked at **50 text items** and 120 KiB per request, so a pathological capture degrades instead of failing |
| Errors | 401/403 → credential/host hint, 456 → quota, 429 → rate limit, others → engine message; all flattened to one line (no stack traces, no multi-line bodies) |
| Transport | `ureq` + rustls, 30 s timeout (`DEEPL_TIMEOUT_SECS`) — thin, blocking, easy to test |

The HTTP layer is deliberately two pure functions (`request_body`,
`parse_response`) plus one `map_status`, so request shape, response parsing and
error mapping are unit-tested against fixtures. One integration test runs the
real client against a throwaway local TCP server to prove the wire format,
including the 51-items-becomes-two-requests case. The real API is never called in
tests.

## OCR

`crates/ocr` shells out to the `tesseract` CLI:

```
tesseract <image> stdout -l <langs> --psm 3 tsv
```

Why the CLI and not a binding: no leptonica/C build on maripop, language packs are
ordinary apt packages, and a crash in OCR cannot take the UI down. The TSV parser
is pure and fixture-tested.

- One `TextBlock` per **line** (words grouped by block/paragraph/line), bounding
  box = union of the word boxes, confidence = mean word confidence.
- Lines below 30 % confidence are dropped.
- CJK words are joined without spaces; everything else with spaces.
- Languages: all 23 supported source packs are installed, and the default `-l` is
  the full list. Narrow it with `COSMIC_SCREENSHOT_OCR_LANGS=eng+deu` or
  `--ocr-langs eng+deu` when speed matters — multi-language Tesseract is slower
  than a two-language run.
- `tesseract` missing is `OCR engine not configured: …`, one line, not a panic.

## Overlay

`crates/overlay` is pure pixel work on an RGBA buffer (no compositor, no shaders),
which makes it deterministic and testable:

1. **Cover**: the OCR box plus a margin proportional to its height (Tesseract
   boxes are tight around glyphs; the original em box is taller). The margin hides
   ascenders, descenders and antialiasing without eating the neighbouring line.
2. **Paper colour**: median colour of the ring just outside the cover — a median,
   not a mean, so a neighbouring glyph or border cannot tint the patch.
3. **Ink colour**: median of the pixels inside the box that differ from the paper
   colour by ≥ 40 luminance; falls back to black/white on a blank box.
4. **Fit**: greedy word wrap (per-character for scripts without spaces), then the
   largest font size whose wrapped block fits the box. A single line is measured
   as one em, not one line advance, so it keeps the original's scale. If even the
   minimum overflows, it is clipped rather than dropped.
5. **Draw**: glyph outlines from `ab_glyph` blended into the buffer, clipped to the
   cover rectangle, so a long translation can never paint over the next block.
6. **Save**: the mutated buffer is written as PNG. That is the composited capture.

Fonts are resolved from the system database by target script (Noto/DejaVu
families); nothing is bundled. Missing fonts produce a message naming the apt
package to install.

## Code map

```
src/main.rs        capture (portal), CLI flags, notify, headless --translate path
src/gui.rs         preview window: Translate, Save, Close, language picker
src/pipeline.rs    capture file → OCR → translate → overlay → PNG (shared by both)
src/lang.rs        one table: code, display name, Tesseract pack, overlay script
crates/translation DeepL client, batching, errors, fake translator for tests
crates/ocr         tesseract runner + TSV parser
crates/overlay     colour sampling, text fitting, in-place rendering
examples/prototype static-image demo: real Tesseract + dictionary fake, no key
tests/             pipeline and CLI contract (one-line errors, no output on failure)
```

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `DEEPL_API_KEY` | — | required for translation; set on the machine, never in the repo |
| `DEEPL_HOST` | `free` | `free` (`api-free.deepl.com`), `paid` (`api.deepl.com`), or a full base URL |
| `DEEPL_TIMEOUT_SECS` | `30` | HTTP timeout |
| `TRANSLATE_PROVIDER` | `deepl` | `deepl`; other enum values report "not implemented yet" |
| `COSMIC_SCREENSHOT_OCR_LANGS` | all supported packs | Tesseract `-l` list |

## Known limits / follow-ups

- The selection UI is the portal's; the Translate button acts on the region the
  portal returned. A button *inside* the portal's selection overlay would require
  replacing `xdg-desktop-portal-cosmic` — deliberately out of scope.
- One-shot overlay: translating again re-runs from the original capture (the
  translated file is never fed back in as input).
- Copy-to-clipboard from the preview is not wired up yet; Save writes the file.
- DeepL free tier is 500k characters/month; a screen region is usually a few
  hundred characters, so this is thousands of captures.
- RTL layout (Arabic) is drawn left-to-right with a font that has the glyphs;
  proper bidi shaping is a follow-up.
- No per-block manual editing (move/resize/retype) yet.
