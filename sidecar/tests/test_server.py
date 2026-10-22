import json
import threading
import urllib.error
import urllib.request
from pathlib import Path

import pytest

from nllb_sidecar.detect import FixedDetector
from nllb_sidecar.engine import EngineError
from nllb_sidecar.languages import LanguageTable
from nllb_sidecar.server import BadRequest, Sidecar, serve

TABLE = Path(__file__).resolve().parents[1] / "nllb_sidecar" / "data" / "nllb_codes.json"


class FakeEngine:
    model_dir = "/models/nllb"
    device = "cpu"
    compute_type = "int8"
    threads = 8

    def __init__(self, fail: str | None = None, count: int | None = None):
        self.fail = fail
        self.count = count
        self.calls: list[tuple[list[str], str, str]] = []

    def translate(self, lines, target_code, source_code):
        self.calls.append((list(lines), target_code, source_code))
        if self.fail:
            raise EngineError(self.fail)
        if self.count is not None:
            return [f"t{i}" for i in range(self.count)]
        return [f"[{target_code}] {line}" for line in lines]


@pytest.fixture(scope="module")
def table() -> LanguageTable:
    return LanguageTable.load(TABLE)


def sidecar(table, engine=None, detector=None) -> Sidecar:
    return Sidecar(engine or FakeEngine(), table, detector or FixedDetector("de"), "fixed")


def test_all_lines_go_through_one_engine_call(table):
    engine = FakeEngine()
    body = sidecar(table, engine).translate(
        {"lines": ["Hallo", "Welt"], "target": "en"}
    )
    assert body["translations"] == ["[eng_Latn] Hallo", "[eng_Latn] Welt"]
    assert body["source"] == "de"
    assert body["detected"] is True
    assert len(engine.calls) == 1, "one call per capture, not one per line"
    assert engine.calls[0] == (["Hallo", "Welt"], "eng_Latn", "deu_Latn")


def test_an_explicit_source_bypasses_detection(table):
    engine = FakeEngine()
    detector = FixedDetector("fr")
    body = sidecar(table, engine, detector).translate(
        {"lines": ["こんにちは"], "target": "en", "source": "ja"}
    )
    assert body["source"] == "ja"
    assert body["detected"] is False
    assert engine.calls[0][2] == "jpn_Jpan"


def test_auto_is_the_default_source(table):
    engine = FakeEngine()
    body = sidecar(table, engine).translate({"lines": ["Hallo"], "target": "en", "source": "auto"})
    assert body["source"] == "de"
    assert engine.calls[0][2] == "deu_Latn"


def test_text_already_in_the_target_language_is_unchanged(table):
    engine = FakeEngine()
    body = sidecar(table, engine, FixedDetector("de")).translate(
        {"lines": ["Schon deutsch"], "target": "de"}
    )
    assert body["translations"] == ["Schon deutsch"]
    assert body["unchanged"] is True
    assert engine.calls == [], "an already-translated capture must not cost 0.6 s"


def test_chinese_traditional_target_is_a_real_translation(table):
    engine = FakeEngine()
    detector = FixedDetector("zh")
    body = sidecar(table, engine, detector).translate(
        {"lines": ["這是一個繁體中文的句子"], "target": "zh"}
    )
    assert body["unchanged"] is False
    assert engine.calls[0] == (["這是一個繁體中文的句子"], "zho_Hans", "zho_Hant")


def test_an_unknown_target_is_rejected_with_a_list(table):
    with pytest.raises(BadRequest) as error:
        sidecar(table).translate({"lines": ["Hallo"], "target": "tlh"})
    assert error.value.code == "unknown_language"
    assert "unknown target language 'tlh'" in str(error.value)
    assert "de" in error.value.details["supported"]


def test_an_unknown_source_is_rejected(table):
    with pytest.raises(BadRequest) as error:
        sidecar(table).translate({"lines": ["Hallo"], "target": "en", "source": "tlh"})
    assert error.value.code == "unknown_language"
    assert "unknown source language" in str(error.value)


@pytest.mark.parametrize(
    "payload",
    [
        {},
        {"lines": "Hallo", "target": "en"},
        {"lines": [], "target": "en"},
        {"lines": [1], "target": "en"},
        {"lines": ["Hallo"]},
        {"lines": ["Hallo"], "target": ""},
        {"lines": ["Hallo"], "target": "en", "source": 3},
    ],
)
def test_malformed_requests_are_bad_requests(table, payload):
    with pytest.raises(BadRequest):
        sidecar(table).translate(payload)


def test_an_engine_failure_is_reported(table):
    engine = FakeEngine(fail="CUDA out of memory")
    with pytest.raises(BadRequest) as error:
        sidecar(table, engine).translate({"lines": ["Hallo"], "target": "en"})
    assert error.value.code == "engine_failed"
    assert "translation failed: CUDA out of memory" in str(error.value)


def test_a_short_answer_is_a_count_mismatch(table):
    with pytest.raises(BadRequest) as error:
        sidecar(table, FakeEngine(count=1)).translate({"lines": ["a", "b"], "target": "en"})
    assert error.value.code == "count_mismatch"


def test_health_reports_the_engine(table):
    health = sidecar(table).health()
    assert health["ok"] is True
    assert health["device"] == "cpu"
    assert health["compute_type"] == "int8"
    assert health["languages"] == len(table.languages)
    assert health["detector"] == "fixed"


@pytest.fixture()
def running(table):
    server = serve(sidecar(table), "127.0.0.1", 0)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def post(url, payload, raw=None):
    data = raw if raw is not None else json.dumps(payload).encode()
    request = urllib.request.Request(
        url, data=data, headers={"Content-Type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        return response.status, json.loads(response.read())


def test_http_translate_and_health(running):
    status, body = post(f"{running}/translate", {"lines": ["Hallo"], "target": "en"})
    assert status == 200
    assert body["translations"] == ["[eng_Latn] Hallo"]

    with urllib.request.urlopen(f"{running}/health", timeout=5) as response:
        health = json.loads(response.read())
    assert health["ok"] is True


def test_http_errors_are_json_with_one_line(running):
    with pytest.raises(urllib.error.HTTPError) as error:
        post(f"{running}/translate", {"lines": ["Hallo"], "target": "tlh"})
    assert error.value.code == 400
    body = json.loads(error.value.read())
    assert "\n" not in body["error"]
    assert body["code"] == "unknown_language"


def test_http_invalid_json_is_a_400(running):
    with pytest.raises(urllib.error.HTTPError) as error:
        post(f"{running}/translate", None, raw=b"{not json")
    assert error.value.code == 400


def test_http_unknown_endpoint_is_a_404(running):
    with pytest.raises(urllib.error.HTTPError) as error:
        urllib.request.urlopen(f"{running}/nope", timeout=5)
    assert error.value.code == 404
