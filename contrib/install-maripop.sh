#!/bin/sh
# Build and install the translation fork on maripop.
#
#   ./contrib/install-maripop.sh
#
# Installs the OCR runtime and the fonts the overlay needs, builds the binary
# and puts it in ~/.local/bin. It does NOT install the translation engine (run
# contrib/install-nllb-sidecar.sh for that) and it does NOT touch the stock
# cosmic-screenshot package.
set -eu

REPO="${REPO:-https://github.com/MarioCakeDev/cosmic-screenshot}"
SOURCE_DIR="${SOURCE_DIR:-$HOME/src/cosmic-screenshot}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

# OCR: base engine + every language pack the tool can read as a source.
# Fonts: DejaVu covers Latin/Cyrillic/Greek; Noto CJK covers ja/zh/ko;
# Noto core covers Arabic.
OCR_PACKAGES="tesseract-ocr tesseract-ocr-osd
tesseract-ocr-ara tesseract-ocr-ces tesseract-ocr-chi-sim tesseract-ocr-chi-tra
tesseract-ocr-dan tesseract-ocr-deu tesseract-ocr-ell tesseract-ocr-eng
tesseract-ocr-fin tesseract-ocr-fra tesseract-ocr-hun tesseract-ocr-ind
tesseract-ocr-ita tesseract-ocr-jpn tesseract-ocr-kor tesseract-ocr-nld
tesseract-ocr-pol tesseract-ocr-por tesseract-ocr-ron tesseract-ocr-rus
tesseract-ocr-spa tesseract-ocr-swe tesseract-ocr-tur tesseract-ocr-ukr"

FONT_PACKAGES="fonts-dejavu-core fonts-noto-core fonts-noto-cjk"

echo "== installing OCR runtime and fonts"
# shellcheck disable=SC2086
sudo apt-get update -qq
# shellcheck disable=SC2086
sudo apt-get install -y --no-install-recommends $OCR_PACKAGES $FONT_PACKAGES

echo "== fetching source"
if [ -d "$SOURCE_DIR/.git" ]; then
    git -C "$SOURCE_DIR" pull --ff-only
else
    mkdir -p "$(dirname "$SOURCE_DIR")"
    git clone -b feat/lens-translation "$REPO" "$SOURCE_DIR"
fi

echo "== building"
cd "$SOURCE_DIR"
cargo build --release

echo "== installing to $BIN_DIR"
mkdir -p "$BIN_DIR"
install -Dm0755 target/release/cosmic-screenshot-translate "$BIN_DIR/cosmic-screenshot-translate"

echo
echo "Installed: $BIN_DIR/cosmic-screenshot-translate"
echo "Stock cosmic-screenshot is untouched."
echo
echo "Next: install the local NLLB-200 engine (no key, ~670 MB, systemd user unit):"
echo "  $SOURCE_DIR/contrib/install-nllb-sidecar.sh"
echo "then verify with:"
echo "  cosmic-screenshot-translate --input shot.png --translate de"
