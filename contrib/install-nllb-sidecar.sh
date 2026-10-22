#!/bin/sh
# Install the local NLLB-200 sidecar on maripop.
#
#   ./contrib/install-nllb-sidecar.sh
#
# User-local: no sudo, no system packages, no API key. Installs a virtualenv,
# the model and tokenizer (~670 MB), fastText detection, and a systemd *user*
# unit that keeps the model resident. Re-running is safe.
set -eu

REPO="${SOURCE_DIR:-$HOME/src/cosmic-screenshot}"
ROOT="$HOME/.local/share/cosmic-screenshot/nllb"
UNIT_DIR="$HOME/.config/systemd/user"
PYTHON="${PYTHON:-python3}"
URL="http://127.0.0.1:8765"

if ! command -v "$PYTHON" >/dev/null 2>&1; then
    echo "python3 not found; install python3 and python3-venv first" >&2
    exit 1
fi
if [ ! -d "$REPO/sidecar" ]; then
    echo "no sidecar/ directory in $REPO" >&2
    echo "clone the branch first: git clone -b feat/lens-translation https://github.com/MarioCakeDev/cosmic-screenshot" >&2
    exit 1
fi

echo "== sidecar virtualenv and Python dependencies"
mkdir -p "$ROOT"
if [ ! -x "$ROOT/venv/bin/python" ]; then
    "$PYTHON" -m venv "$ROOT/venv"
fi
"$ROOT/venv/bin/pip" install --quiet --upgrade pip wheel
# Installed (not -e), so the service does not depend on the checkout staying put.
"$ROOT/venv/bin/pip" install --quiet "$REPO/sidecar"

echo "== model (~647 MB int8), tokenizer, language detector"
"$ROOT/venv/bin/python" - "$ROOT" <<'PY'
import sys
import urllib.request
from pathlib import Path

from huggingface_hub import snapshot_download

root = Path(sys.argv[1])
snapshot_download(
    "JustFrederik/nllb-200-distilled-600M-ct2-int8",
    local_dir=root / "model",
    allow_patterns=["model.bin", "config.json", "shared_vocabulary.txt"],
)
snapshot_download(
    "facebook/nllb-200-distilled-600M",
    local_dir=root / "tokenizer",
    allow_patterns=[
        "tokenizer.json",
        "tokenizer_config.json",
        "special_tokens_map.json",
        "sentencepiece.bpe.model",
    ],
)
detector = root / "lid.176.ftz"
if not detector.is_file() or detector.stat().st_size < 900_000:
    urllib.request.urlretrieve(
        "https://dl.fbaipublicfiles.com/fasttext/supervised-models/lid.176.ftz",
        detector,
    )
PY

echo "== systemd user unit"
mkdir -p "$UNIT_DIR"
install -m0644 "$REPO/sidecar/systemd/cosmic-screenshot-nllb.service" "$UNIT_DIR/"
systemctl --user daemon-reload
# `enable` alone does not start the unit, and `restart` both starts a stopped
# one and reloads a running one. That way the model is loaded exactly once on a
# first install, and a re-run picks up the freshly installed package instead of
# leaving the old module in memory.
if ! systemctl --user enable cosmic-screenshot-nllb.service ||
   ! systemctl --user restart cosmic-screenshot-nllb.service; then
    echo "could not enable/start the user service; run this inside the desktop session:" >&2
    echo "  systemctl --user enable --now cosmic-screenshot-nllb.service" >&2
    exit 1
fi

echo "== waiting for the model to load"
attempt=0
while [ "$attempt" -lt 30 ]; do
    if curl -fsS -m 2 "$URL/health" >/dev/null 2>&1; then
        break
    fi
    attempt=$((attempt + 1))
    sleep 1
done
if ! curl -fsS -m 5 "$URL/health"; then
    echo >&2
    echo "sidecar did not come up; inspect: journalctl --user -u cosmic-screenshot-nllb -n 40" >&2
    exit 1
fi
echo
echo "Installed. Translate a file:"
echo "  cosmic-screenshot-translate --input shot.png --translate en"
