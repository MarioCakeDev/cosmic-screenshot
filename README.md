# cosmic-screenshot

Utility for capturing screenshots via XDG Desktop Portal.

This fork adds **Lens-style translation** of the captured region: OCR the
selection, translate it **locally with NLLB-200**, and paint the translation
back over the original text.

- Fork of [`pop-os/cosmic-screenshot`](https://github.com/pop-os/cosmic-screenshot)
  (upstream history kept).
- Installs as a **separate binary**, `cosmic-screenshot-translate`; the stock
  `cosmic-screenshot` package is untouched.
- The **Translate button in the screenshot dialog** is not in this repository:
  it lives in the `xdg-desktop-portal-cosmic` fork
  ([`MarioCakeDev/xdg-desktop-portal-cosmic`](https://github.com/MarioCakeDev/xdg-desktop-portal-cosmic)),
  which owns that overlay and its toolbar. Both front ends share the engine in
  `crates/engine`, so there is exactly one copy of the OCR/overlay pipeline.
- Translation runs on the machine: no API key, no cloud, no per-character cost.
  NLLB-200 is a dedicated NMT model, not an LLM — no Ollama, no prompts.
- Reads the capture in two stages: a cheap English **probe** names the language
  (no full 24-pack read, which lets Tesseract read Latin UI text as Arabic or
  CJK), then the main pass reads with that language's packs alone, enlarged 2×
  first. Screenshot page-segmentation modes are compared rather than assuming a
  document. Isolated UI labels remain the weak spot; see the measured limitation
  in [`docs/NLLB.md`](docs/NLLB.md).
- The status line always reports **found / kept / translated / drawn**, so a
  partial capture cannot look like a success, and pressing Translate on a screen
  already in the target language says so instead of repainting the same words.
- Design note: [`docs/DESIGN.md`](docs/DESIGN.md); engine and install detail:
  [`docs/NLLB.md`](docs/NLLB.md).

## Use

```sh
# headless: select a region, translate it, print the saved path
cosmic-screenshot-translate --translate de

# whole screen, no selection UI
cosmic-screenshot-translate --translate ja --interactive=false

# an existing image (also handy for testing without the portal)
cosmic-screenshot-translate --input shot.png --translate fr

# tell it the source language instead of detecting it
cosmic-screenshot-translate --input shot.png --translate en --source-lang de
```

The translated image is written next to the capture as
`<name>.translated.png` (override with `--output`). The saved image contains the
translated overlay, not the original text.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `TRANSLATE_PROVIDER` | `local-nmt` | `local-nmt` (NLLB sidecar) or `deepl` (paid plan) |
| `COSMIC_SCREENSHOT_NLLB_URL` | `http://127.0.0.1:8765` | local sidecar base URL |
| `COSMIC_SCREENSHOT_NLLB_TIMEOUT_SECS` | `30` | sidecar HTTP timeout |
| `DEEPL_API_KEY` | — | only for `TRANSLATE_PROVIDER=deepl` |
| `DEEPL_HOST` | `paid` | `paid` (`api.deepl.com`), `free` (a trailing `:fx` on the key selects it automatically), or a full URL |
| `COSMIC_SCREENSHOT_OCR_LANGS` | all supported packs | Tesseract `-l` list; choosing it disables the probe and the narrowing pass |
| `COSMIC_SCREENSHOT_OCR_BINARIZE` | `1` | Binarise (grey → autocontrast → threshold) before recognition; `0` hands Tesseract the greyscale crop |
| `COSMIC_SCREENSHOT_OCR_THRESHOLD` | `150` | Luma split point for the binarised crop, `1`–`254` |
| `COSMIC_SCREENSHOT_OCR_SPELLFIX` | `1` | Repair tokens one or two edits from a known UI word (`Gack` → `Back`); `0` passes OCR text through untouched |
| `COSMIC_SCREENSHOT_OCR_PSM` | psm 6, fallback 11 | Pin one Tesseract page-segmentation mode (for tuning a capture) |
| `COSMIC_SCREENSHOT_OCR_SCALE` | `2` | Enlargement before recognition, `1`–`4` (`1` disables it) |
| `COSMIC_SCREENSHOT_DEBUG_DIR` | — | Opt-in OCR dump: each run writes the exact crop handed to Tesseract, the raw TSV and the painted result under `run-*` there |

With the sidecar stopped, the Translate button and `--translate` fail with one
line, before the selection UI opens:

```
translation engine not available: cannot reach the local NLLB sidecar at http://127.0.0.1:8765 — start it with 'systemctl --user start cosmic-screenshot-nllb'
```

Target languages: `en de fr es it pt nl pl ru uk tr cs sv da fi el hu ro ja zh
ko ar id`. The **source** language is detected per capture (fastText on all OCR
lines joined); if detection is not confident it falls back to English and says
so. `--source-lang` overrides it on the command line; the dialog always
auto-detects.

## Build on maripop

```sh
sudo apt-get install -y --no-install-recommends \
  tesseract-ocr tesseract-ocr-osd python3 python3-venv \
  tesseract-ocr-ara tesseract-ocr-ces tesseract-ocr-chi-sim tesseract-ocr-chi-tra \
  tesseract-ocr-dan tesseract-ocr-deu tesseract-ocr-ell tesseract-ocr-eng \
  tesseract-ocr-fin tesseract-ocr-fra tesseract-ocr-hun tesseract-ocr-ind \
  tesseract-ocr-ita tesseract-ocr-jpn tesseract-ocr-kor tesseract-ocr-nld \
  tesseract-ocr-pol tesseract-ocr-por tesseract-ocr-ron tesseract-ocr-rus \
  tesseract-ocr-spa tesseract-ocr-swe tesseract-ocr-tur tesseract-ocr-ukr \
  fonts-dejavu-core fonts-noto-core fonts-noto-cjk

git clone -b feat/lens-translation https://github.com/MarioCakeDev/cosmic-screenshot \
  ~/src/cosmic-screenshot
cd ~/src/cosmic-screenshot
cargo build --release
install -Dm0755 target/release/cosmic-screenshot-translate ~/.local/bin/
```

Then install the translation engine (user-local venv, ~670 MB model, systemd
user unit):

```sh
./contrib/install-nllb-sidecar.sh
```

Or run `contrib/install-maripop.sh` for the binary, then the sidecar script.

Verify:

```sh
curl -s http://127.0.0.1:8765/health
cosmic-screenshot-translate --input shot.png --translate de
```

## Tests

```sh
cargo test --workspace            # no network, no model, no key
cargo test --workspace --all-features
cd sidecar && python -m pytest    # engine tests with fixtures (no model)

# with the real model downloaded (proves the CTranslate2 handling):
cd sidecar
NLLB_TEST_MODEL=~/.local/share/cosmic-screenshot/nllb/model \
NLLB_TEST_TOKENIZER=~/.local/share/cosmic-screenshot/nllb/tokenizer \
python -m pytest -m real
```

The HTTP transports are tested against a throwaway local server; no real API is
called. The local engine's language table is checked against the model's 202
language tokens (Python fixture test) and the 23-entry picker is cross-checked
against the same table (Rust test). The `Translator` trait with a fake provider
is the main pipeline test path.
