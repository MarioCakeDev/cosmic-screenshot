"""NLLB-200 via CTranslate2.

The model is loaded once and stays resident: a cold load costs 0.7–2.3 s, which
would be added to every screenshot if this ran per capture. One `translate`
call carries every OCR line of the capture, because CTranslate2 has a fixed
~200 ms per-call overhead — per-line calls would pay it per line.

Two CTranslate2 details that fail *silently* if you get them wrong:

1. `tokenizer.src_lang` must be set as an attribute. Passing `src_lang=` to the
   tokenizer call is ignored, the source is labelled English, and the output is
   fluent nonsense that looks like a broken quantisation.
2. The target prefix is echoed in `hypotheses[0]`; the first token must be
   dropped or every line starts with `eng_Latn`.
"""

from __future__ import annotations

import os
import threading
from pathlib import Path

from .languages import LanguageTable

DEFAULT_THREADS = 8
# Short OCR lines; generous headroom, and EOS ends decoding long before this.
MAX_DECODING_LENGTH = 512


class EngineError(Exception):
    """Anything that stops the engine from loading or translating, as one line."""


def one_line(message: object) -> str:
    """Collapse engine/library multi-line noise into the single line the UI shows."""
    return " ".join(str(message).split())


class NllbEngine:
    def __init__(
        self,
        model_dir: str | Path,
        tokenizer_dir: str | Path,
        table: LanguageTable,
        device: str = "auto",
        compute_type: str = "auto",
        threads: int | None = None,
    ):
        self.model_dir = Path(model_dir)
        self.tokenizer_dir = Path(tokenizer_dir)
        _require_model_files(self.model_dir)
        _require_tokenizer_files(self.tokenizer_dir)

        try:
            import ctranslate2  # noqa: PLC0415
            from transformers import AutoTokenizer  # noqa: PLC0415
        except ImportError as error:
            raise EngineError(f"the sidecar's Python dependencies are incomplete: {error}") from error

        threads = threads if threads and threads > 0 else min(DEFAULT_THREADS, os.cpu_count() or 1)
        try:
            self._tokenizer = AutoTokenizer.from_pretrained(str(self.tokenizer_dir))
            # Trap 1: an attribute, never a call kwarg.
            self._tokenizer.src_lang = table.default_source_code()
            table.validate_against(self._tokenizer.get_vocab())
            self._translator = ctranslate2.Translator(
                str(self.model_dir),
                device=device,
                compute_type=compute_type,
                inter_threads=1,
                intra_threads=threads,
            )
        except Exception as error:  # noqa: BLE001 - one clear line, not a traceback
            raise EngineError(one_line(error)) from error

        self.device = str(getattr(self._translator, "device", device))
        self.compute_type = str(getattr(self._translator, "compute_type", compute_type))
        self.threads = threads
        # `src_lang` is mutated per request, so requests are serialised. One
        # screenshot at a time is the actual workload.
        self._lock = threading.Lock()

    def translate(self, lines: list[str], target_code: str, source_code: str) -> list[str]:
        if not lines:
            return []
        with self._lock:
            try:
                self._tokenizer.src_lang = source_code
                batch = [
                    self._tokenizer.convert_ids_to_tokens(
                        self._tokenizer(line, truncation=True, max_length=1024).input_ids
                    )
                    for line in lines
                ]
                results = self._translator.translate_batch(
                    batch,
                    target_prefix=[[target_code] for _ in batch],
                    beam_size=1,
                    # One call for the whole capture; the internal batch stays
                    # bounded so a dense screenshot cannot balloon memory.
                    max_batch_size=min(len(batch), 32),
                    max_decoding_length=MAX_DECODING_LENGTH,
                )
                return [self._decode(result.hypotheses[0], target_code) for result in results]
            except Exception as error:  # noqa: BLE001
                raise EngineError(one_line(error)) from error

    def _decode(self, tokens: list[str], target_code: str) -> str:
        # Trap 2: the target prefix comes back as the first token.
        if tokens and (tokens[0] == target_code or _is_language_token(tokens[0])):
            tokens = tokens[1:]
        ids = self._tokenizer.convert_tokens_to_ids(tokens)
        return self._tokenizer.decode(ids, skip_special_tokens=True).strip()


def _is_language_token(token: str) -> bool:
    language, _, script = token.partition("_")
    return len(language) == 3 and language.isalpha() and len(script) == 4 and script.isalpha()


def _require_model_files(model_dir: Path) -> None:
    missing = [name for name in ("model.bin", "config.json") if not (model_dir / name).is_file()]
    if missing:
        raise EngineError(
            f"NLLB model not found at {model_dir} (missing {', '.join(missing)})"
        )


def _require_tokenizer_files(tokenizer_dir: Path) -> None:
    has_tokenizer = (tokenizer_dir / "tokenizer.json").is_file()
    has_sentencepiece = (tokenizer_dir / "sentencepiece.bpe.model").is_file()
    if not (has_tokenizer and has_sentencepiece):
        raise EngineError(f"NLLB tokenizer not found at {tokenizer_dir} (expected tokenizer.json and sentencepiece.bpe.model)")
