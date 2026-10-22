# Local NLLB-200 translation engine

The translation engine is **local NLLB-200-distilled-600M**, served by a small
Python sidecar on `127.0.0.1`. No cloud engine, no API key, no DeepL, and no
Ollama: NLLB is a purpose-built neural machine translation model, not an LLM
prompt.

600M is the default because it is half the size of 1.3B and better on weak
machines. The model directory is configurable (`NLLB_MODEL_DIR`), and the 1.3B
distilled model is a drop-in upgrade for full sentences — at the cost of worse
results on isolated UI labels (see [Model size](#model-size-600m-default-13b-optional)).

Why a sidecar instead of Rust bindings (`ct2rs`): bindings would drag
`libctranslate2` onto every target machine, which is worse for "runnable on a
laptop" than a pip-installable service. The model also has to stay resident (a
cold load is 0.7–2.3 s), which is exactly what a systemd user unit is for.

## Measured

maripop: Ryzen 7 5800X, AVX2 only. NLLB-200-distilled-600M, CTranslate2 int8:

| Config | 25 lines / 1121 ch | 5 lines / 184 ch | RSS |
|---|---|---|---|
| CPU 8 threads, beam 4 (default) | 1.82 s | – | 1.5 GB |
| CPU 8 threads, beam 1 | 0.63 s | 274 ms | 1.4 GB |
| CPU 4 threads, beam 1 | – | 318 ms | 1.4 GB |
| CPU 2 threads, beam 1 | 1.40 s | 498 ms | 1.4 GB |
| CPU 1 thread, beam 1 | 2.56 s | 887 ms | 1.4 GB |
| GPU int8_float16, beam 1 | 0.06 s | 39 ms | 1.3 GB RSS, 1.20 GB VRAM |

Beam 4 costs about 3× on CPU (0.63 s → 1.82 s for 25 lines) and almost nothing
on a GPU. It is worth it: see [Accuracy](#accuracy-beam-size-and-short-labels).

Disk: ~670 MB (int8 model, tokenizer, detector). No GPU required.

The application sends **all OCR lines of one capture in one request** and the
model stays loaded; both follow from the fixed ~200 ms per-call overhead and the
cold-load cost above.

## Accuracy: beam size and short labels

Decoding runs at **beam 4** by default (`NLLB_BEAM_SIZE`, `--beam-size`). Beam 1
is not merely "slightly worse" on a screenshot: an isolated UI label gives the
decoder almost no context, and greedy search completes a memorised sentence
instead of translating the label.

Measured on the installed 600M model, beam 1:

| Source | beam 1 | beam 4 |
|---|---|---|
| Abbrechen | The following is the list of the countries of the European Union: | Withdrawals |
| Einstellungen | The following is the list of the categories of services: | Settings |
| Abmelden | The following information shall be provided: | Withdrawal |

That lands in a button-sized box on the screenshot, which is the worst possible
failure for this feature. Beam 4 avoids most of it; the rest is the model's real
limitation (below).

As a backstop, the engine refuses to paint an implausible result for a short
line: for a line of **1–3 tokens**, a translation more than **3× the input
token count** is treated as a hallucination and the **original text is kept**.
Longer lines are never second-guessed — their length follows from their content,
and dropping them would hide real translations.

## Model size: 600M default, 1.3B optional

The default is `nllb-200-distilled-600M` (int8, ~647 MB): half the download and
the better choice on a weak machine. The model directory is not hard-coded —
`NLLB_MODEL_DIR` (or `--model-dir`) points the sidecar at any CTranslate2
conversion of a compatible NLLB-200 model, and the tokenizer stays where it is:

```sh
# any CTranslate2 int8 conversion of NLLB-200-distilled-1.3B
systemctl --user edit cosmic-screenshot-nllb.service
# [Service]
# Environment=NLLB_MODEL_DIR=%h/.local/share/cosmic-screenshot/nllb/model-1.3B
systemctl --user restart cosmic-screenshot-nllb.service
curl -s http://127.0.0.1:8765/health   # "model": ".../model-1.3B"
```

Be honest about the trade-off, measured at beam 4:

| | 600M | 1.3B |
|---|---|---|
| `Speichern` | Storage | – |
| `Einstellungen` | Settings (beam 4) | Employment |
| `Bearbeiten` | Manufacture | Manufacture |
| `Hilfe` | – | Aid |
| `Zurück` | "I'm going back" | – |
| `Weiter` | "Going on" | – |
| "Mit freundlichen Grüßen" | With a friendly greeting | With warm regards |
| "Rücksendungen sind innerhalb von 14 Tagen kostenlos" | Free return within 14 days | Returns are free within 14 days |

**Short UI labels are unreliable at any beam size and with either model.** Do
not treat a label translation as authoritative; full sentences are fine, and
1.3B is clearly better on them. The short-line guard above keeps the worst
hallucinations off the screenshot, but a wrong-but-short label (Storage for
Speichern) can still appear.

Translating the whole capture as one newline-joined block does **not** work:
the model merges and drops lines (6 in, 1 out, on both 600M and 1.3B).
Line-by-line in one batched call is the workable shape.

## Two-pass OCR

Reading a capture whose language is unknown means giving Tesseract every pack.
That is the right first move but it costs accuracy on the language it turns out
to be: with all 24 packs, plain German is misread — `für` → `fur`,
`Rücksendungen` → `Ricksendungen` — and the translation then follows the
misreading ("The programme is free of charge for 14 days" instead of "Free
return within 14 days").

So the pipeline reads the capture twice:

1. OCR with the full pack list → fastText detection on that text → source
   language (e.g. `de`, 0.991);
2. OCR **again** with only the detected language's pack (`deu`) → translate
   those lines.

The second pass costs one extra Tesseract run (~200 ms) and removes a whole
class of wrong translations. If the second pass fails or finds nothing (for
example a capture that mixes scripts), the first pass is used and translation
continues — the accuracy pass is never a requirement.

`--ocr-langs eng+deu` (or `COSMIC_SCREENSHOT_OCR_LANGS`) is the manual override:
choosing packs explicitly turns the second pass off, because then the user, not
the detector, decides what Tesseract reads.

The sidecar exposes detection as `POST /detect` for this. With an older sidecar
that predates the endpoint, the client gets a 404 and silently falls back to
one-pass OCR, so a stale sidecar never breaks a capture — it just loses the
second pass. Reinstall the sidecar after pulling to get it — re-run
`contrib/install-nllb-sidecar.sh`, which reinstalls the package *and restarts
the unit* (see [Install](#install-on-maripop)). Engines that cannot detect
without translating (DeepL reports the source only in its translation response)
skip the pass.

## Install on maripop

Prerequisites: `python3` and `python3-venv` (`sudo apt-get install -y python3
python3-venv`) and this repository checked out at
`~/src/cosmic-screenshot` on `feat/lens-translation`.

One command:

```sh
./contrib/install-nllb-sidecar.sh
```

It installs the venv, the model, the tokenizer, `lid.176.ftz`, the unit, enables
and restarts the service, and waits for `/health`. Re-running it after a `git
pull` reinstalls the Python package and restarts the unit, so a pulled fix
actually reaches the running service. The same steps by hand:

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

CPU takes 274 ms for 5 lines at beam 1 and ~3× that at beam 4; a GPU takes
39 ms at beam 1 and very little more at beam 4. To use it:

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
 "source": "de", "source_confidence": 0.995,
 "detected": true, "fallback": false, "unchanged": false}
```

- `source` is `"auto"` (detect) or a two-letter code.
- `detected` is true when a confidence-bearing detection produced `source`;
  `fallback` is true when detection was not trusted and `source` is the
  configured default instead. They are never both true, and an explicit
  `source` sets both to false.
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
- `GET /health` returns model path, device, compute type, thread count, beam
  size and the number of languages.

`POST /detect` returns the source language without translating; the desktop
client uses it for the second OCR pass:

```json
{"lines": ["Rücksendungen sind kostenlos"]}
```

```json
{"source": "de", "source_confidence": 0.991, "detected": true, "fallback": false}
```

It takes the same `lines` validation and reports the same `detected`/`fallback`
fields as `/translate`. An unknown or missing `source` is not an error: the
client then keeps its one-pass OCR.

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
`"detected": false` with `"fallback": true`; the notification and the preview
status then say **English (assumed)** rather than presenting it as a detection.
`--source-lang de` and the preview window's **From** picker skip detection
entirely (`detected` and `fallback` are both false, and the chosen language is
shown as the source). A trusted detection also drives the second OCR pass
([Two-pass OCR](#two-pass-ocr)); a low-confidence fallback never does.

## Environment variables (sidecar)

| Variable | Default | Meaning |
|---|---|---|
| `NLLB_MODEL_DIR` | `~/.local/share/cosmic-screenshot/nllb/model` | CTranslate2 model directory (point it at a 1.3B conversion to upgrade) |
| `NLLB_TOKENIZER_DIR` | `…/nllb/tokenizer` | NLLB tokenizer files (shared by 600M and 1.3B) |
| `NLLB_FASTTEXT_MODEL` | `…/nllb/lid.176.ftz` | language detector |
| `NLLB_LANG_TABLE` | packaged `data/nllb_codes.json` | override the ISO→NLLB table |
| `NLLB_DEVICE` | `auto` | `cpu`, `cuda`, `auto` |
| `NLLB_COMPUTE_TYPE` | `auto` | `int8`, `int8_float16`, … |
| `NLLB_THREADS` | `8` | CPU intra-op threads |
| `NLLB_BEAM_SIZE` | `4` | decoding beam width (1–10); 1 is faster but hallucinates on short labels |
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
| A short label is replaced by a long sentence | beam size is 1: remove `NLLB_BEAM_SIZE=1` / `--beam-size 1`, restart the unit, check `/health` shows `"beam_size": 4` |
| Accents are misread (`für` → `fur`) and the translation follows | the second OCR pass did not run: `--ocr-langs`/`COSMIC_SCREENSHOT_OCR_LANGS` is set (it disables the pass), or the installed sidecar predates `/detect` — reinstall it |

## Quality

Good but literal, clearly below DeepL:

| Source | NLLB-200 | DeepL |
|---|---|---|
| Die Lieferung erfolgt innerhalb von drei Werktagen | Delivery shall be made within three working days | Your delivery will arrive within three working days |
| Mit freundlichen Grüßen | With a friendly greeting | Kind regards |

Do not oversell it in UI copy or docs. Short UI labels are the weak spot and
stay unreliable at any beam size — see
[Model size](#model-size-600m-default-13b-optional).

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
5. `beam_size` is **4**, not 1: greedy decoding invents whole sentences for
   isolated labels (see [Accuracy](#accuracy-beam-size-and-short-labels)).
6. `numpy<2` is pinned: `fasttext-wheel` 0.9.2 calls `np.array(..., copy=False)`,
   removed in NumPy 2, which makes every detection raise `ValueError`.
