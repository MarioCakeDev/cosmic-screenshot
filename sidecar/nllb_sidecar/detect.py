"""Source-language detection for one capture.

NLLB has no language detector: it needs an explicit `src_lang` token. fastText's
`lid.176.ftz` (917 KB, 176 languages) is used here. It is deliberately run once
per *capture*, on all OCR lines joined together — a single short line ("Date:",
"3 Stück") is genuinely ambiguous and per-line detection produces a different
language for every line.

Low confidence, an unknown label, or an empty capture does not fail the
translation: it falls back to a configured default and says so, so the caller
can show "detected language" honestly. An explicit source language from the
user bypasses detection entirely.
"""

from __future__ import annotations

import contextlib
import io
from dataclasses import dataclass
from pathlib import Path

from .languages import LanguageTable, normalise_iso

# Beyond this, labels get no more reliable and only cost time.
MAX_DETECT_CHARS = 4000


@dataclass(frozen=True)
class Detection:
    iso: str
    confidence: float
    #: True when detection was not trusted and `iso` is the configured default.
    used_default: bool
    #: The raw fastText label, for the log line only.
    label: str | None = None


def join_lines(lines: list[str]) -> str:
    """One detection input per capture, not one per line."""
    return " ".join(line.strip() for line in lines if line.strip())[:MAX_DETECT_CHARS]


class Detector:
    """fastText detector. The import is lazy so the rest stays testable."""

    def __init__(self, model_path: str | Path, table: LanguageTable, model=None):
        self._table = table
        if model is not None:  # injection point for tests
            self._model = model
        else:
            import fasttext  # noqa: PLC0415 - only needed at runtime

            # fasttext-wheel prints a deprecation notice about the returned
            # object on every load; it is not actionable, keep the journal clean.
            with contextlib.redirect_stderr(io.StringIO()):
                self._model = fasttext.load_model(str(model_path))

    def detect(self, lines: list[str]) -> Detection:
        text = join_lines(lines)
        if not text:
            return Detection(self._table.default_source, 0.0, True)
        labels, scores = self._model.predict(text.replace("\n", " "), k=1)
        if not labels:
            return Detection(self._table.default_source, 0.0, True)
        label = labels[0]
        confidence = float(scores[0])
        iso = normalise_iso(label)
        known = self._table.code(iso) is not None
        if not known or confidence < self._table.min_confidence:
            return Detection(self._table.default_source, confidence, True, label)
        return Detection(iso, confidence, False, label)


class FixedDetector:
    """Deterministic detector for tests and for `--source-lang`-only setups."""

    def __init__(self, iso: str, confidence: float = 1.0):
        self.iso = iso
        self.confidence = confidence

    def detect(self, _lines: list[str]) -> Detection:
        return Detection(self.iso, self.confidence, False)


class DefaultDetector:
    """Always reports the configured default; used when fastText is absent."""

    def __init__(self, table: LanguageTable):
        self._table = table

    def detect(self, _lines: list[str]) -> Detection:
        return Detection(self._table.default_source, 0.0, True)


def detector_from_path(model_path: str | Path | None, table: LanguageTable):
    """A fastText detector when the model exists, the default detector otherwise."""
    if model_path is None:
        return DefaultDetector(table)
    path = Path(model_path)
    if not path.is_file():
        return DefaultDetector(table)
    return Detector(path, table)
