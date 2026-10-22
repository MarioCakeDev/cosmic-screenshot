# cosmic-screenshot

Utility for capturing screenshots via XDG Desktop Portal.

This fork adds **Lens-style translation** of the captured region: OCR the
selection, translate it with DeepL, and paint the translation back over the
original text.

- Fork of [`pop-os/cosmic-screenshot`](https://github.com/pop-os/cosmic-screenshot)
  (upstream history kept).
- Installs as a **separate binary**, `cosmic-screenshot-translate`; the stock
  `cosmic-screenshot` package is untouched.
- Design note: [`docs/DESIGN.md`](docs/DESIGN.md).

## Use

```sh
# select a region, then get a preview with a Translate button and language picker
cosmic-screenshot-translate

# headless: select a region, translate it, print the saved path
cosmic-screenshot-translate --translate de

# whole screen, no selection UI
cosmic-screenshot-translate --translate ja --interactive=false

# an existing image (also handy for testing without the portal)
cosmic-screenshot-translate --input shot.png --translate fr
```

The translated image is written next to the capture as
`<name>.translated.png` (override with `--output`). The saved image contains the
translated overlay, not the original text.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `DEEPL_API_KEY` | — | DeepL key, read at runtime; never committed |
| `DEEPL_HOST` | `free` | `free` → `api-free.deepl.com`, `paid` → `api.deepl.com`, or a full base URL |
| `TRANSLATE_PROVIDER` | `deepl` | other engines are expressible but not implemented yet |
| `COSMIC_SCREENSHOT_OCR_LANGS` | all supported packs | Tesseract `-l` list |

Without `DEEPL_API_KEY` the Translate button and `--translate` fail with one line:

```
translation engine not configured: set DEEPL_API_KEY
```

Target and source languages: `en de fr es it pt nl pl ru uk tr cs sv da fi el hu
ro ja zh ko ar id`. The source language is detected by DeepL; all OCR language
packs for those languages are installed by the recipe below.

## Build on maripop

```sh
sudo apt-get install -y --no-install-recommends \
  tesseract-ocr tesseract-ocr-osd \
  tesseract-ocr-ara tesseract-ocr-ces tesseract-ocr-chi-sim tesseract-ocr-chi-tra \
  tesseract-ocr-dan tesseract-ocr-deu tesseract-ocr-ell tesseract-ocr-eng \
  tesseract-ocr-fin tesseract-ocr-fra tesseract-ocr-hun tesseract-ocr-ind \
  tesseract-ocr-ita tesseract-ocr-jpn tesseract-ocr-kor tesseract-ocr-nld \
  tesseract-ocr-pol tesseract-ocr-por tesseract-ocr-ron tesseract-ocr-rus \
  tesseract-ocr-spa tesseract-ocr-swe tesseract-ocr-tur tesseract-ocr-ukr \
  fonts-dejavu-core fonts-noto-core fonts-noto-cjk

git clone https://github.com/MarioCakeDev/cosmic-screenshot
cd cosmic-screenshot
cargo build --release
install -Dm0755 target/release/cosmic-screenshot-translate ~/.local/bin/
```

Or run `contrib/install-maripop.sh`, which does exactly that (without the key).

The key belongs in the session environment, not the repository:

```sh
install -Dm600 /dev/null ~/.config/environment.d/90-deepl.conf
printf 'DEEPL_API_KEY=%s\n' '<free-tier-key>' >> ~/.config/environment.d/90-deepl.conf
```

Log out and back in, then verify:

```sh
cosmic-screenshot-translate --input shot.png --translate de
```

## Tests

```sh
cargo test --workspace            # 60 tests, no network, no API key
cargo test --workspace --all-features
cargo run --example prototype -- /tmp/translate-demo   # needs tesseract-ocr + eng pack
```

The DeepL client is tested with fixture request/response bodies and against a
throwaway local HTTP server; the real API is never called. The `Translator` trait
with a fake provider is the main test path for the pipeline.
