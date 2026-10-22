# Design note — Lens-style translation of a captured region

Fork: <https://github.com/MarioCakeDev/cosmic-screenshot> (upstream history kept,
nothing pushed to `pop-os`).

## What the tool does

1. Capture a region through the XDG desktop portal — the stock `cosmic-screenshot`
   selection UI, unchanged.
2. OCR the captured image and keep each recognised **line** with its box.
3. Translate every line in **one** request to a **local NLLB-200 sidecar**
   (source language detected once per capture with fastText).
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

## Translation: vendor-neutral, local NLLB-200 by default

`crates/translation` exposes the only interface the pipeline knows:

```rust
trait Translator {
    fn provider(&self) -> Provider;
    fn translate(&self, texts: &[&str], target: Lang, source: Option<Lang>)
        -> Result<Batch, TranslateError>;
}
```

`Provider` is an enum over `DeepL | GoogleCloud | Azure | LocalNmt`. `LocalNmt`
is the default: NLLB-200-distilled-600M served by a local sidecar, no API key,
no cloud, no per-character cost. `DeepL` is kept behind the same trait for
anyone with a paid DeepL plan; Google and Azure remain placeholders that fail
with one clear line. Adding a provider is one `impl Translator`; nothing in OCR
or overlay changes.

### Why a sidecar rather than Rust bindings

`ct2rs` would drag `libctranslate2` onto every target machine — worse for
"runnable on a laptop" than a pip-installable service. The sidecar also has to
hold the model **resident**: a cold load is 0.7–2.3 s, so a process per capture
would add that to every screenshot.

Measured on maripop (Ryzen 7 5800X, AVX2 only), NLLB-200-distilled-600M,
CTranslate2 int8:

| Config | 25 lines / 1121 ch | 5 lines / 184 ch | RSS |
|---|---|---|---|
| CPU 8 threads, beam 4 (default) | 1.82 s | – | 1.5 GB |
| CPU 8 threads, beam 1 | 0.63 s | 274 ms | 1.4 GB |
| CPU 4 threads, beam 1 | – | 318 ms | 1.4 GB |
| CPU 1 thread, beam 1 | 2.56 s | 887 ms | 1.4 GB |
| GPU int8_float16, beam 1 | 0.06 s | 39 ms | 1.3 GB RSS, 1.20 GB VRAM |

Three consequences, all encoded in the code:

1. **One request per capture.** There is a fixed ~200 ms per-call overhead; the
   client never chunks (that is a DeepL-only limit).
2. **Model resident** in a systemd *user* unit, started at login.
3. **`beam_size = 4`** — 3× the CPU cost of beam 1, but greedy decoding invents
   whole sentences for isolated UI labels. A length guard keeps the original
   text for a 1–3 token line whose translation comes back implausibly long.

### Protocol

| Endpoint | Body |
|---|---|
| `POST /translate` | `{"lines": ["…"], "target": "de", "source": "auto" \| "de"}` → `{"translations": ["…"], "source": "de", "detected": true, "unchanged": false}` |
| `POST /detect` | `{"lines": ["…"]}` → `{"source": "de", "detected": true, "fallback": false}` — used for the second OCR pass |
| `GET /health` | model path, device, compute type, thread count, beam size, language count |

Canonical two-letter codes cross the wire; the NLLB token (`deu_Latn`,
`arb_Arab`, `zho_Hans`) exists in exactly one place, the sidecar's
`data/nllb_codes.json`. A code the model does not know is a 400, never a silent
translation into the wrong language.

### Language codes and detection

- NLLB needs the **script** as part of the token: `arb_Arab` for Arabic,
  `lvs_Latn` for Latvian, `zho_Hans`/`zho_Hant` for Chinese. Naive ISO 639-3
  guesses are wrong for several picker languages, so the table is checked
  against the model vocabulary **at startup** (the service refuses to run), by a
  Python fixture test against the 202 real tokens, and by a Rust test that
  cross-checks the 23-entry picker against the same table.
- **Detection is new work DeepL hid:** NLLB has no detector. fastText
  `lid.176.ftz` (917 KB) runs **once per capture on all joined lines**; per-line
  detection is unreliable on short strings. Low confidence or an unknown label
  falls back to English (reported as `detected: false`), and `--source-lang` /
  the preview's **From** picker bypass detection entirely.
- Chinese is the one language whose script fastText cannot report: it returns
  `zh` for both, so the sidecar picks Hans/Hant from the characters themselves.

### DeepL specifics (optional engine)

| Concern | Decision |
|---|---|
| Endpoint | `https://api.deepl.com` (paid) is the default; a key ending in `:fx` selects `https://api-free.deepl.com`; `DEEPL_HOST` overrides either; any full URL accepted for proxies/tests |
| Key | `DEEPL_API_KEY`, read at runtime; never logged, never committed, redacted in `Debug` |
| Not configured | `translation engine not configured: set DEEPL_API_KEY` — one line, exit 1, no capture taken |
| Source language | omitted from the request; DeepL's `detected_source_language` is surfaced in the status |
| Batching | 50 items and 120 KiB per request — DeepL's limits, not NLLB's |
| Errors | 401/403 → credential/host hint, 456 → quota, 429 → rate limit, all one line |
| Transport | `ureq` + rustls, 30 s timeout (`DEEPL_TIMEOUT_SECS`) |

The HTTP layers are pure functions (`request_body`, `parse_response`,
`map_status`) tested against fixtures, plus integration tests against a
throwaway local TCP server. No real API is called in tests.

### Failure shape

One line, never a panic and never a silent empty overlay:

```
translation engine not available: cannot reach the local NLLB sidecar at http://127.0.0.1:8765 — start it with 'systemctl --user start cosmic-screenshot-nllb'
translation engine not available: NLLB model not found at /home/…/nllb/model (missing model.bin, config.json)
```

The client checks `/health` **before** the portal opens, so a stopped engine
never costs the user a region selection.

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
  the full list. The capture is then read **twice**: full list → fastText
  detection → the detected language's pack alone (`deu`), because all packs at
  once misreads accents (`für` → `fur`, `Rücksendungen` → `Ricksendungen`) and
  the translation follows the misreading. `--ocr-langs eng+deu` /
  `COSMIC_SCREENSHOT_OCR_LANGS` is the manual override and turns the second pass
  off. A failed or empty second pass keeps the first reading; a mixed-script
  capture can come back partial, with the other script's lines left untranslated.
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
src/gui.rs         preview window: Translate, Save, Close, From/To pickers
src/pipeline.rs    capture file → OCR → translate → overlay → PNG (shared by both)
src/lang.rs        one table: code, display name, Tesseract pack, overlay script
crates/translation local NLLB client, DeepL client, errors, fake translator for tests
crates/ocr         tesseract runner + TSV parser
crates/overlay     colour sampling, text fitting, in-place rendering
sidecar/           NLLB-200 + CTranslate2 + fastText service, its own tests and unit
examples/prototype static-image demo: real Tesseract + dictionary fake, no engine
tests/             pipeline, CLI contract, picker ↔ NLLB code cross-check
```

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `TRANSLATE_PROVIDER` | `local-nmt` | `local-nmt` (default) or `deepl`; other enum values report "not implemented yet" |
| `COSMIC_SCREENSHOT_NLLB_URL` | `http://127.0.0.1:8765` | sidecar base URL |
| `COSMIC_SCREENSHOT_NLLB_TIMEOUT_SECS` | `30` | sidecar HTTP timeout |
| `DEEPL_API_KEY` | — | only for `TRANSLATE_PROVIDER=deepl` |
| `DEEPL_HOST` | `paid` | `paid` (`api.deepl.com`), `free` (a trailing `:fx` key selects it automatically), or a full base URL |
| `DEEPL_TIMEOUT_SECS` | `30` | DeepL HTTP timeout |
| `COSMIC_SCREENSHOT_OCR_LANGS` | all supported packs | Tesseract `-l` list; choosing it disables the automatic second OCR pass |

The sidecar has its own settings (`NLLB_MODEL_DIR`, `NLLB_TOKENIZER_DIR`,
`NLLB_FASTTEXT_MODEL`, `NLLB_DEVICE`, `NLLB_COMPUTE_TYPE`, `NLLB_THREADS`,
`NLLB_BEAM_SIZE`, `NLLB_PORT`); see [`docs/NLLB.md`](NLLB.md).

## Known limits / follow-ups

- The selection UI is the portal's; the Translate button acts on the region the
  portal returned. A button *inside* the portal's selection overlay would require
  replacing `xdg-desktop-portal-cosmic` — deliberately out of scope.
- One-shot overlay: translating again re-runs from the original capture (the
  translated file is never fed back in as input).
- Copy-to-clipboard from the preview is not wired up yet; Save writes the file.
- **NLLB quality is good but literal, clearly below DeepL** ("Mit freundlichen
  Grüßen" → "With a friendly greeting"). The engine model is 669 MB on disk and
  about 1.5 GB RSS at the default beam 4 while resident. **Isolated UI labels
  are unreliable at any beam size and with either model** (`Einstellungen` →
  Settings on 600M, Employment on 1.3B); the length guard keeps whole-sentence
  hallucinations off the screenshot, but a wrong short label can still appear.
  Full sentences are fine. Measured table: [`docs/NLLB.md`](NLLB.md).
- RTL layout (Arabic) is drawn left-to-right with a font that has the glyphs;
  proper bidi shaping is a follow-up.
- No per-block manual editing (move/resize/retype) yet.
