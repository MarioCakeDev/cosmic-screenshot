# Local NLLB-200 translation engine

The translation engine is **local NLLB-200-distilled-600M**, served by a small
Python sidecar on `127.0.0.1`. No cloud engine, no API key, no DeepL, and no
Ollama: NLLB is a purpose-built neural machine translation model, not an LLM
prompt.

Why a sidecar instead of Rust bindings (`ct2rs`): bindings would drag
`libctranslate2` onto every target machine, which is worse for "runnable on a
laptop" than a pip-installable service. The model also has to stay resident (a
cold load is 0.7–2.3 s), which is exactly what a systemd user unit is for.

## Measured

maripop: Ryzen 7 5800X, AVX2 only. NLLB-200-distilled-600M, CTranslate2 int8,
beam 1:

| Config | 25 lines / 1121 ch | 5 lines / 184 ch | RSS |
|---|---|---|---|
| CPU 8 threads | 0.63 s | 274 ms | 1.4 GB |
| CPU 4 threads | – | 318 ms | 1.4 GB |
| CPU 2 threads | 1.40 s | 498 ms | 1.4 GB |
| CPU 1 thread | 2.56 s | 887 ms | 1.4 GB |
| GPU int8_float16 | 0.06 s | 39 ms | 1.3 GB RSS, 1.20 GB VRAM |

Disk: ~670 MB (int8 model, tokenizer, detector). No GPU required.

The application sends **all OCR lines of one capture in one request** and the
model stays loaded; both follow from the fixed ~200 ms per-call overhead and the
cold-load cost above.

## Install on maripop

Prerequisites: `python3` and `python3-venv` (`sudo apt-get install -y python3
python3-venv`) and this repository checked out at
`~/src/cosmic-screenshot` on `feat/lens-translation`.

One command:

```sh
./contrib/install-nllb-sidecar.sh
```

It installs the venv, the model, the tokenizer, `lid.176.ftz`, the unit, enables
and starts the service, and waits for `/health`. The same steps by hand:

```sh
# 1. virtualenv + Python dependencies
ROOT="$HOME/.local/share/cosmic-screenshot/nllb"
python3 -m venv "$ROOT/venv"
"$ROOT/venv/bin/pip" install --upgrade pip wheel
"$ROOT/venv/bin/pip" install ~/src/cosmic-screenshot/sidecar

# 2. model (647 MB int8), tokenizer, detector
"$ROOT/venv/bin/python" - "$ROOT" <<'PY'
import sys, urllib.request
from pathlib import Path
from huggingface_hub import snapshot_download
root = Path(sys.argv[1])
snapshot_download("JustFrederik/nllb-200-distilled-600M-ct2-int8",
                  local_dir=root / "model",
                  allow_patterns=["model.bin", "config.json", "shared_vocabulary.txt"])
snapshot_download("facebook/nllb-200-distilled-600M",
                  local_dir=root / "tokenizer",
                  allow_patterns=["tokenizer.json", "tokenizer_config.json",
                                  "special_tokens_map.json", "sentencepiece.bpe.model"])
urllib.request.urlretrieve(
    "https://dl.fbaipublicfiles.com/fasttext/supervised-models/lid.176.ftz",
    root / "lid.176.ftz")
PY

# 3. systemd user unit
mkdir -p ~/.config/systemd/user
install -m644 ~/src/cosmic-screenshot/sidecar/systemd/cosmic-screenshot-nllb.service \
    ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now cosmic-screenshot-nllb.service

# 4. verify
curl -s http://127.0.0.1:8765/health
```

Expected layout:

```
~/.local/share/cosmic-screenshot/nllb/
├── venv/
├── model/        model.bin (647 MB), config.json, shared_vocabulary.txt
├── tokenizer/    tokenizer.json, sentencepiece.bpe.model, tokenizer_config.json,
│                 special_tokens_map.json
└── lid.176.ftz   917 KB fastText language detector
```

The unit runs
`~/.local/share/cosmic-screenshot/nllb/venv/bin/python -m nllb_sidecar`, binds
`127.0.0.1:8765` only, restarts on failure, and stops trying on exit code 78
(`EX_CONFIG`, e.g. a missing model) instead of restart-looping.

### GPU (optional)

CPU takes 274 ms for 5 lines; a GPU takes 39 ms. To use it:

```sh
systemctl --user edit cosmic-screenshot-nllb.service
# [Service]
# Environment=NLLB_DEVICE=cuda
# Environment=NLLB_COMPUTE_TYPE=int8_float16
systemctl --user restart cosmic-screenshot-nllb.service
curl -s http://127.0.0.1:8765/health   # "device": "cuda"
```

## Verify end to end

```sh
curl -s http://127.0.0.1:8765/health
curl -s -X POST http://127.0.0.1:8765/translate \
  -H 'Content-Type: application/json' \
  -d '{"lines":["Die Lieferung erfolgt innerhalb von drei Werktagen."],"target":"en"}'
# {"translations":["Delivery shall be made within three working days."],"source":"de",...}

cosmic-screenshot-translate --input shot.png --translate en
tesseract shot.translated.png stdout   # the painted text is English
```

Engine tests (fixtures only, no model):

```sh
cd sidecar && pip install -e '.[test]' && python -m pytest
```

Real-model tests (what proves the two CTranslate2 traps are handled):

```sh
cd sidecar
NLLB_TEST_MODEL="$HOME/.local/share/cosmic-screenshot/nllb/model" \
NLLB_TEST_TOKENIZER="$HOME/.local/share/cosmic-screenshot/nllb/tokenizer" \
python -m pytest -m real
```

## Protocol

`POST /translate`

```json
{"lines": ["Die Lieferung erfolgt", "innerhalb von drei Werktagen."],
 "target": "en", "source": "auto"}
```

```json
{"translations": ["Delivery takes place", "within three working days."],
 "source": "de", "source_confidence": 0.995, "detected": true, "unchanged": false}
```

- `source` is `"auto"` (detect) or a two-letter code.
- The wire carries **ISO 639-1**, not NLLB tokens. The token (`deu_Latn`) is
  resolved inside the sidecar from one table, so a wrong code is a 400 and can
  never silently translate into another language.
- `target: "zh"` means simplified Chinese (`zho_Hans`); there is one Chinese
  entry in the picker, as with the previous engine.
- If the detected/declared source equals the target, the lines come back
  unchanged with `"unchanged": true` (a German capture translated to German
  costs 0 ms instead of 0.6 s).
- A request is one capture: at most **500 lines**, 20 000 characters per line and
  1 MiB of JSON. Beyond that the sidecar answers 400 rather than splitting the
  capture, because chunking would pay the per-call overhead again — a capture
  that large is not a screenshot of a paragraph.
- `GET /health` returns model path, device, compute type, thread count and the
  number of languages.

## Language table and Chinese script

`sidecar/nllb_sidecar/data/nllb_codes.json` maps 109 ISO codes to NLLB tokens.
It is checked in three places:

1. at startup against the **loaded tokenizer's vocabulary** — the service
   refuses to run if a code is unknown;
2. by `sidecar/tests/test_languages.py` against the 202 real NLLB language
   tokens (`tests/fixtures/nllb_language_tokens.txt`, extracted from
   `facebook/nllb-200-distilled-600M/tokenizer.json`);
3. by the Rust test `tests/nllb_languages.rs`, which cross-checks the 23-entry
   picker against the same table.

Several codes are *not* the naive ISO 639-3 guess: Arabic is `arb_Arab` (not
`ara_Arab`), Latvian is `lvs_Latn` (not `lav_Latn`), Albanian is `als_Latn`,
Malay is `zsm_Latn`, Persian is `pes_Arab`. That is why the table is validated
rather than derived.

Chinese needs a script subtag that fastText cannot report: it returns `zh` for
both. The sidecar therefore reads traditional/simplified markers from the
capture itself (`zho_Hant` vs `zho_Hans`), defaulting to simplified when there
is no evidence either way.

## Detection

NLLB has no detector — this is work DeepL used to do for free. fastText
`lid.176.ftz` runs **once per capture**, on all OCR lines joined together:
single short lines are ambiguous. Below the confidence threshold (`0.4`) or on a
label the table does not know, the sidecar falls back to English and reports
`"detected": false`, which the UI shows as the source language rather than
pretending. `--source-lang de` and the preview window's **From** picker skip
detection entirely.

## Environment variables (sidecar)

| Variable | Default | Meaning |
|---|---|---|
| `NLLB_MODEL_DIR` | `~/.local/share/cosmic-screenshot/nllb/model` | CTranslate2 model directory |
| `NLLB_TOKENIZER_DIR` | `…/nllb/tokenizer` | NLLB tokenizer files |
| `NLLB_FASTTEXT_MODEL` | `…/nllb/lid.176.ftz` | language detector |
| `NLLB_LANG_TABLE` | packaged `data/nllb_codes.json` | override the ISO→NLLB table |
| `NLLB_DEVICE` | `auto` | `cpu`, `cuda`, `auto` |
| `NLLB_COMPUTE_TYPE` | `auto` | `int8`, `int8_float16`, … |
| `NLLB_THREADS` | `8` | CPU intra-op threads |
| `NLLB_HOST` / `NLLB_PORT` | `127.0.0.1` / `8765` | listen address |
| `NLLB_DEFAULT_SOURCE` | `en` | fallback when detection is not trusted |
| `NLLB_MIN_CONFIDENCE` | `0.4` | below this, use the default source |

Client side: `COSMIC_SCREENSHOT_NLLB_URL` (default `http://127.0.0.1:8765`) and
`COSMIC_SCREENSHOT_NLLB_TIMEOUT_SECS` (default `30`).

## Troubleshooting

| What you see | Meaning / fix |
|---|---|
| `translation engine not available: cannot reach the local NLLB sidecar at http://127.0.0.1:8765 — start it with 'systemctl --user start cosmic-screenshot-nllb'` | service stopped: `systemctl --user start cosmic-screenshot-nllb`, then `journalctl --user -u cosmic-screenshot-nllb -n 40` |
| `translation engine not available: NLLB model not found at … (missing model.bin, config.json)` | model download incomplete; re-run the install script |
| `translation engine not available: NLLB tokenizer not found at …` | tokenizer files missing; re-run the install script |
| `translation engine not available: fastText language model not found at …` | rerun the install script, or run with `--no-detect` and a fixed `--source-lang` |
| `translation engine rejected the request (400): unknown target language 'xx'` | client sent a code the table does not have; add it to `nllb_codes.json` **and** the picker, or fix the picker |
| `EADDRINUSE` / cannot listen | another process holds 8765: set `NLLB_PORT` and the matching `COSMIC_SCREENSHOT_NLLB_URL` |
| English output from a non-English capture | detection fell back: check `/health` shows `"detector": "fasttext"`, or pass `--source-lang` |

## Quality

Good but literal, clearly below DeepL:

| Source | NLLB-200 | DeepL |
|---|---|---|
| Die Lieferung erfolgt innerhalb von drei Werktagen | Delivery shall be made within three working days | Your delivery will arrive within three working days |
| Mit freundlichen Grüßen | With a friendly greeting | Kind regards |

Do not oversell it in UI copy or docs.

## CTranslate2 traps this sidecar handles

1. `tokenizer.src_lang` is set as an **attribute**. As a `__call__` kwarg it is
   silently ignored, the source is labelled English, and the output is fluent
   English nonsense that looks like a broken quantisation.
2. CTranslate2 echoes the `target_prefix` language token in `hypotheses[0]`;
   the first token is dropped before decoding, otherwise every line starts with
   `eng_Latn`.
3. The pre-quantised int8 repository is used (`…-ct2-int8`, 647 MB): same speed
   and quality as quantising float16 at load, half the download.
4. Tokenizer files come from `facebook/nllb-200-distilled-600M`; the
   pre-quantised repository is only used for `model.bin`.
5. `beam_size=1`.
6. `numpy<2` is pinned: `fasttext-wheel` 0.9.2 calls `np.array(..., copy=False)`,
   removed in NumPy 2, which makes every detection raise `ValueError`.
