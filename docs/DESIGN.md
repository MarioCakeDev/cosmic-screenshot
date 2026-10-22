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
5. Show the result in the dialog that is already open — the **Translate** button
   sits in the screenshot toolbar next to Capture and the save destination, and
   the preview image is replaced in place. Save/Copy then export the composited
   PNG, not the original text.

`--translate <language>` runs steps 1–4 headlessly and prints the output path.

## Where the button lives (and why)

`cosmic-screenshot` upstream is a ~150-line CLI: it asks the portal for a
screenshot and saves the file. **The area-selection UI is not in this repository**
— it belongs to `xdg-desktop-portal-cosmic`, which the whole desktop uses.

The button therefore lives in a fork of that portal
([`MarioCakeDev/xdg-desktop-portal-cosmic`](https://github.com/MarioCakeDev/xdg-desktop-portal-cosmic)),
in `src/widget/screenshot.rs`'s toolbar and `src/screenshot.rs`'s dialog state.
There is no second window: the dialog already holds the captured image, the
selection and the toolbar, and it already produces the bytes that Save and Copy
export. The fork replaces the captured image's buffer before that export, so the
exported file is the translated one by construction.

The earlier preview window (`src/gui.rs`, iced + tiny-skia) is gone. It read as a
workaround because it was one.

## Shared engine

`crates/engine` is the single implementation of OCR → detect → translate →
overlay. The CLI and the portal dialog both call
`engine::translate_canvas`, so the probe-and-narrow OCR decision, the language table and
the overlay call cannot drift between them. The portal vendors these crates
(`crates/{ocr,overlay,translation,engine}`) with their source commit recorded in
`crates/VENDORED.md`; `contrib/sync-engine.sh` refreshes them.

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
  falls back to English (reported as `detected: false`), and `--source-lang`
  bypasses detection entirely.
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
tesseract <image> stdout -l <langs> --psm 6 tsv
```

Why the CLI and not a binding: no leptonica/C build on maripop, language packs are
ordinary apt packages, and a crash in OCR cannot take the UI down. The TSV parser
is pure and fixture-tested.

- One `TextBlock` per **label**, not per Tesseract line. Level-5 word rows are
  filtered to tokens that contain a letter (a button border read as `|` is
  dropped), then clustered by horizontal gap: a gap wider than twice the line's
  median word gap — or wider than 1.2 line heights — starts a new block, and
  each block's box is the padded union of **its words**. This matters twice
  over: the line box starts at the button border (~410 px left of the words on
  the Minecraft capture), and psm 6 merges two button columns into one line, so
  a line-box fill covers the art between them and the translation lands far left
  of its label. Word clustering fixes placement, fill width and column merging
  in one change; psm 11 still returns one line per row, so it is not the fix.
- The crop is binarised before recognition: grey, histogram stretch
  (autocontrast), polarity normalised, then a hard threshold at 150. Polarity
  matters because the threshold calls everything at or above it *paper*: a crop
  whose median luma says the panel is dark is inverted first, so dim secondary
  text stays ink instead of being folded into the background and both themes
  read with one threshold. Game UI is anti-aliased light
  text on a dark panel, and Tesseract reads the soft edges as strokes —
  `Advancements` comes back as `AYancernrentS` on the raw pixels and reads
  correctly binarised. `COSMIC_SCREENSHOT_OCR_BINARIZE=0` and
  `COSMIC_SCREENSHOT_OCR_THRESHOLD` exist for tuning.
- The binarised crop is enlarged 2× with **nearest neighbour** (not Lanczos),
  because a pixel font is already a grid of hard-edged blocks and interpolation
  turns each stroke into a grey ramp the threshold erodes unevenly (`B`/`G`/`F`
  become each other). Boxes are mapped back to capture pixels afterwards. 3×
  nearest and thresholds 130/150/170 were measured on a 1350×1100 pixel-font
  proxy: 3× cost about 40 % more Tesseract time for no consistent gain, so 2× at
  threshold 150 stays the default; `COSMIC_SCREENSHOT_OCR_SCALE=3` pins 3× for a
  real crop.
- One uniform-block pass (psm 6) reads the menu; a sparse-text pass (psm 11) is
  tried only when it keeps fewer than two lines. The old three-mode tournament
  cost six seconds and read no better than one binarised pass.
- Blocks below 30 % confidence are dropped, as are blocks with fewer than three
  letters that are not a known short label (`OK`, `No`, `Ja`, `Sí`, `Da`, `Ya`,
  `Да`, `Не`, `Ні`, or a two-character kana/Han/Hangul word such as `はい`) or
  mostly punctuation — a textured button must not become a translated line, and
  an icon read as `Ei` must not become a word.
- An optional UI-vocabulary corrector repairs a token that is one edit from a
  known menu word (`Gack` → `Back`), or two edits when an adjacent token makes a
  known phrase (`bane Menu` → `Game Menu`, `Fieport Bugs` → `Report Bugs`).
  It runs only on a line that already contains an unambiguous English UI word
  (`OK`, `Sound`, `Video`, `Feedback`, `Credits` and `Mods` are also German and
  do not count), so a German line (`Musik`, `Menü`, `Optionen`) is never
  rewritten into English look-alikes. Tokens shorter than four letters, punctuation-bearing tokens and
  vocabulary words are never touched, so `zu` in a German menu stays `zu`.
  Every repair is logged in `report.txt` (`corrected: 'Gack' -> 'Back' (1 edit)`);
  `COSMIC_SCREENSHOT_OCR_SPELLFIX=0` turns it off.
- CJK words are joined without spaces; everything else with spaces.
- Languages: a cheap `eng` probe names the source first; the main read then uses
  that language's packs (plus `eng` for Latin/Cyrillic/Greek screens that mix
  English labels). Reading with the full list first let Tesseract pick an
  unrelated script per word (`Game` → `سس`). The probe is trusted only when it
  kept real Latin text; the 24-pack read is a fallback reserved for a probe
  that kept fewer than two lines or read no Latin text, because it costs about
  ten times as much. A low-confidence but readable Latin probe is translated
  as read, and the translator detects the source again. `--ocr-langs eng+deu` /
  `COSMIC_SCREENSHOT_OCR_LANGS` is the manual override and skips the probe. A
  failed or empty main pass keeps the probe; a mixed-script capture can come
  back partial, with the other script's lines left untranslated.
- `tesseract` missing is `OCR engine not configured: …`, one line, not a panic.

## Overlay

`crates/overlay` is pure pixel work on an RGBA buffer (no compositor, no shaders),
which makes it deterministic and testable:

1. **Cover**: the label's word-cluster box plus a margin proportional to its
   height (Tesseract boxes are tight around glyphs; the original em box is
   taller). The margin hides ascenders, descenders and antialiasing without
   eating the neighbouring line — and because the box starts at the first word,
   it no longer starts at the button border.
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
src/pipeline.rs    capture file → engine → PNG (the CLI's job description)
crates/engine      OCR → detect → translate → overlay, shared with the portal
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
| `COSMIC_SCREENSHOT_OCR_BINARIZE` | `1` | Binarise (grey → autocontrast → threshold) before recognition |
| `COSMIC_SCREENSHOT_OCR_THRESHOLD` | `150` | Luma split point for the binarised crop, applied after dark-panel inversion so ink is always the dark side, `1`–`254` |
| `COSMIC_SCREENSHOT_OCR_SPELLFIX` | `1` | English-gated UI-vocabulary corrector; `0` passes OCR text through untouched |

The sidecar has its own settings (`NLLB_MODEL_DIR`, `NLLB_TOKENIZER_DIR`,
`NLLB_FASTTEXT_MODEL`, `NLLB_DEVICE`, `NLLB_COMPUTE_TYPE`, `NLLB_THREADS`,
`NLLB_BEAM_SIZE`, `NLLB_PORT`); see [`docs/NLLB.md`](NLLB.md).

## Known limits / follow-ups

- The Translate button replaces the dialog's captured image in place; there is
  no separate preview. Save/Copy export whatever the dialog holds, so they
  export the translated image.
- One-shot overlay: translating again starts from the untouched capture (the
  translated buffer is never fed back in as input), so changing the target
  language does not compound two overlays.
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
