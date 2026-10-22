"""HTTP front end for the NLLB sidecar.

Endpoints:

* `GET  /health`    — model, device and language count; used by the install check
* `POST /translate` — `{"lines": [...], "target": "de", "source": "auto" | "de"}`
                      → `{"translations": [...], "source": "de", ...}`

Only loopback is expected (the systemd unit binds 127.0.0.1). Errors are JSON
with a single-line `error`, so the desktop client can print one line instead of
a traceback.
"""

from __future__ import annotations

import json
import logging
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .detect import Detection, join_lines
from .engine import EngineError, one_line
from .languages import LanguageTable, LanguageTableError, normalise_iso, resolve_source

MAX_BODY_BYTES = 1024 * 1024
MAX_LINES = 500
MAX_LINE_CHARS = 20_000

log = logging.getLogger("nllb_sidecar")


class BadRequest(Exception):
    def __init__(self, message: str, code: str = "bad_request", details: dict | None = None):
        super().__init__(message)
        self.code = code
        self.details = details or {}


class Sidecar:
    """The transport-independent part: request in, (status, body) out."""

    def __init__(self, engine, table: LanguageTable, detector, detector_name: str = "default"):
        self.engine = engine
        self.table = table
        self.detector = detector
        self.detector_name = detector_name

    def health(self) -> dict:
        return {
            "ok": True,
            "model": str(getattr(self.engine, "model_dir", "")),
            "device": str(getattr(self.engine, "device", "unknown")),
            "compute_type": str(getattr(self.engine, "compute_type", "unknown")),
            "threads": int(getattr(self.engine, "threads", 0)),
            "languages": len(self.table.languages),
            "detector": self.detector_name,
        }

    def translate(self, payload: object) -> dict:
        lines, target, source = _validate(payload, self.table)

        detection: Detection | None = None
        source_iso = source
        if source is None:
            detection = self.detector.detect(lines)
            source_iso = detection.iso
        try:
            source_code = resolve_source(self.table, source_iso, join_lines(lines))
            target_code = self.table.require(target)
        except LanguageTableError as error:
            raise BadRequest(one_line(error), "unknown_language") from error

        if source_code == target_code:
            # Already in the target language: nothing useful to translate, and
            # NLLB would only echo it back after 0.6 s.
            return _response(lines, source_iso, detection, unchanged=True)

        try:
            translations = self.engine.translate(lines, target_code, source_code)
        except EngineError as error:
            raise BadRequest(f"translation failed: {one_line(error)}", "engine_failed") from error

        if len(translations) != len(lines):
            raise BadRequest(
                f"translation failed: the engine returned {len(translations)} lines for {len(lines)}",
                "count_mismatch",
            )
        return _response(translations, source_iso, detection)


def _validate(payload: object, table: LanguageTable) -> tuple[list[str], str, str | None]:
    if not isinstance(payload, dict):
        raise BadRequest("the request body must be a JSON object")
    lines = payload.get("lines")
    if not isinstance(lines, list) or not lines:
        raise BadRequest("'lines' must be a non-empty array of strings")
    if len(lines) > MAX_LINES:
        raise BadRequest(f"too many lines: {len(lines)} (maximum {MAX_LINES})")
    for line in lines:
        if not isinstance(line, str):
            raise BadRequest("'lines' must contain only strings")
        if len(line) > MAX_LINE_CHARS:
            raise BadRequest(f"a line is longer than {MAX_LINE_CHARS} characters")

    target = payload.get("target")
    if not isinstance(target, str) or not target.strip():
        raise BadRequest("'target' must be a language code such as 'de'")
    target = normalise_iso(target)
    if table.code(target) is None:
        raise BadRequest(
            f"unknown target language '{target}'",
            "unknown_language",
            {"supported": sorted(table.languages)},
        )

    source = payload.get("source", "auto")
    if source is None or (isinstance(source, str) and source.strip().lower() == "auto"):
        source = None
    elif not isinstance(source, str) or not source.strip():
        raise BadRequest("'source' must be 'auto' or a language code such as 'de'")
    else:
        source = normalise_iso(source)
        if table.code(source) is None:
            raise BadRequest(
                f"unknown source language '{source}'",
                "unknown_language",
                {"supported": sorted(table.languages)},
            )
    return list(lines), target, source


def _response(
    translations: list[str],
    source_iso: str,
    detection: Detection | None,
    unchanged: bool = False,
) -> dict:
    return {
        "translations": list(translations),
        "source": source_iso,
        "source_confidence": round(float(detection.confidence) if detection else 1.0, 3),
        "detected": bool(detection and not detection.used_default),
        "unchanged": unchanged,
    }


def make_handler(sidecar: Sidecar):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "cosmic-screenshot-nllb/0.1"

        def do_GET(self) -> None:  # noqa: N802 - http.server API
            if self.path.rstrip("/") == "/health":
                self._send(200, sidecar.health())
            else:
                self._send(404, {"error": f"no such endpoint: {self.path}"})

        def do_POST(self) -> None:  # noqa: N802 - http.server API
            if self.path.rstrip("/") != "/translate":
                self._send(404, {"error": f"no such endpoint: {self.path}"})
                return
            started = time.monotonic()
            try:
                payload = self._read_json()
                body = sidecar.translate(payload)
                status = 200
            except BadRequest as error:
                body = {"error": one_line(error), "code": error.code, **error.details}
                # 503: the engine exists but cannot serve right now — the same
                # "not available" class as a refused connection on the client.
                status = 503 if error.code == "engine_failed" else 400
            except Exception as error:  # noqa: BLE001 - the service must not die
                log.exception("unhandled request error")
                body = {"error": f"internal error: {one_line(error)}", "code": "internal"}
                status = 500
            elapsed_ms = (time.monotonic() - started) * 1000
            log.info(
                "%s %s -> %s %.0f ms",
                self.client_address[0],
                self.path,
                status,
                elapsed_ms,
            )
            self._send(status, body)

        def _read_json(self) -> object:
            raw_length = self.headers.get("Content-Length")
            if raw_length is None:
                raise BadRequest("missing Content-Length")
            try:
                length = int(raw_length)
            except ValueError as error:
                raise BadRequest("invalid Content-Length") from error
            if length < 0 or length > MAX_BODY_BYTES:
                raise BadRequest(f"request body larger than {MAX_BODY_BYTES} bytes")
            raw = self.rfile.read(length)
            try:
                return json.loads(raw.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise BadRequest(f"invalid JSON body: {error}") from error

        def _send(self, status: int, body: dict) -> None:
            raw = json.dumps(body, ensure_ascii=False).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, format: str, *args) -> None:  # noqa: A002
            log.debug("%s - %s", self.address_string(), format % args)

    return Handler


def serve(sidecar: Sidecar, host: str, port: int) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer((host, port), make_handler(sidecar))
    server.daemon_threads = True
    return server
