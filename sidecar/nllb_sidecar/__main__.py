"""Start the local NLLB translation service.

    python -m nllb_sidecar

Every startup failure is a single line on stderr beginning with
`translation engine not available: ` and exit code 78 (EX_CONFIG), so a user
service does not restart-loop on a missing model.
"""

from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path

from .detect import DefaultDetector, Detector
from .engine import EngineError, NllbEngine, one_line
from .languages import LanguageTable, LanguageTableError
from .server import Sidecar, serve

EX_CONFIG = 78
DEFAULT_PORT = 8765
DEFAULT_HOST = "127.0.0.1"


def default_data_dir() -> Path:
    root = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share")
    return root / "cosmic-screenshot" / "nllb"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    data_dir = default_data_dir()
    parser = argparse.ArgumentParser(
        prog="nllb_sidecar",
        description="Local NLLB-200 translation service for cosmic-screenshot-translate",
    )
    parser.add_argument("--host", default=os.environ.get("NLLB_HOST", DEFAULT_HOST))
    parser.add_argument("--port", type=int, default=int(os.environ.get("NLLB_PORT", DEFAULT_PORT)))
    parser.add_argument(
        "--model-dir", default=os.environ.get("NLLB_MODEL_DIR", str(data_dir / "model"))
    )
    parser.add_argument(
        "--tokenizer-dir",
        default=os.environ.get("NLLB_TOKENIZER_DIR", str(data_dir / "tokenizer")),
    )
    parser.add_argument(
        "--fasttext-model",
        default=os.environ.get("NLLB_FASTTEXT_MODEL", str(data_dir / "lid.176.ftz")),
    )
    parser.add_argument(
        "--lang-table", default=os.environ.get("NLLB_LANG_TABLE") or None
    )
    parser.add_argument("--device", default=os.environ.get("NLLB_DEVICE", "auto"))
    parser.add_argument("--compute-type", default=os.environ.get("NLLB_COMPUTE_TYPE", "auto"))
    parser.add_argument("--threads", type=int, default=int(os.environ.get("NLLB_THREADS", "0")))
    parser.add_argument(
        "--default-source", default=os.environ.get("NLLB_DEFAULT_SOURCE") or None,
        help="language used when detection is not confident (default: from the table)",
    )
    parser.add_argument(
        "--min-confidence", type=float, default=None,
        help="fastText confidence below which the default source is used "
             "(default: NLLB_MIN_CONFIDENCE or the table's value)",
    )
    parser.add_argument(
        "--no-detect", action="store_true",
        help="run without fastText; 'auto' then always means --default-source",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    # transformers prints a "no PyTorch" advisory on import; the tokenizer does
    # not need any of those frameworks.
    os.environ.setdefault("TRANSFORMERS_VERBOSITY", "error")
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    try:
        return _run(args)
    except (EngineError, LanguageTableError) as error:
        print(f"translation engine not available: {one_line(error)}", file=sys.stderr)
        return EX_CONFIG


def min_confidence_from(flag_value: float | None, environ=os.environ) -> float | None:
    """`--min-confidence` wins over `NLLB_MIN_CONFIDENCE`; both are validated."""
    if flag_value is not None:
        value = flag_value
    else:
        raw = (environ.get("NLLB_MIN_CONFIDENCE") or "").strip()
        if not raw:
            return None
        try:
            value = float(raw)
        except ValueError as error:
            raise LanguageTableError(
                f"NLLB_MIN_CONFIDENCE='{raw}' is not a number"
            ) from error
    if not 0.0 <= value <= 1.0:
        raise LanguageTableError("min-confidence must be between 0 and 1")
    return value


def _run(args: argparse.Namespace) -> int:
    table = LanguageTable.load(args.lang_table)
    if args.default_source:
        source = args.default_source.strip().lower()
        table.require(source)
        table = LanguageTable(table.languages, source, table.min_confidence)
    confidence = min_confidence_from(args.min_confidence)
    if confidence is not None:
        table = LanguageTable(table.languages, table.default_source, confidence)

    engine = NllbEngine(
        args.model_dir,
        args.tokenizer_dir,
        table,
        device=args.device,
        compute_type=args.compute_type,
        threads=args.threads,
    )

    fasttext_path = Path(args.fasttext_model)
    if args.no_detect:
        detector = DefaultDetector(table)
        detector_name = "default"
        logging.warning("fastText detection disabled; 'auto' uses source '%s'", table.default_source)
    elif fasttext_path.is_file():
        detector = Detector(fasttext_path, table)
        detector_name = "fasttext"
    else:
        raise EngineError(f"fastText language model not found at {fasttext_path}")

    sidecar = Sidecar(engine, table, detector, detector_name)
    try:
        server = serve(sidecar, args.host, args.port)
    except OSError as error:
        raise EngineError(f"cannot listen on {args.host}:{args.port}: {one_line(error)}") from error

    logging.info(
        "listening on http://%s:%d (model %s, %s/%s, %d threads, %d languages, detector %s)",
        args.host,
        server.server_port,
        args.model_dir,
        engine.device,
        engine.compute_type,
        engine.threads,
        len(table.languages),
        detector_name,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        logging.info("stopping")
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
