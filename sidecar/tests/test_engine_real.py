"""End-to-end translation through the real model.

Skipped unless the model and tokenizer are pointed at explicitly:

    NLLB_TEST_MODEL=~/.local/share/cosmic-screenshot/nllb/model \
    NLLB_TEST_TOKENIZER=~/.local/share/cosmic-screenshot/nllb/tokenizer \
    pytest sidecar/tests -m real

This is the test that catches the two silent CTranslate2 traps: a `src_lang`
that was ignored (fluent English nonsense) and an unstripped target prefix.
"""

import os
from pathlib import Path

import pytest

from nllb_sidecar.engine import NllbEngine
from nllb_sidecar.languages import LanguageTable, resolve_source

MODEL = os.environ.get("NLLB_TEST_MODEL")
TOKENIZER = os.environ.get("NLLB_TEST_TOKENIZER")

pytestmark = [
    pytest.mark.real,
    pytest.mark.skipif(not (MODEL and TOKENIZER), reason="set NLLB_TEST_MODEL and NLLB_TEST_TOKENIZER"),
]

TABLE = Path(__file__).resolve().parents[1] / "nllb_sidecar" / "data" / "nllb_codes.json"


@pytest.fixture(scope="module")
def engine() -> NllbEngine:
    return NllbEngine(MODEL, TOKENIZER, LanguageTable.load(TABLE), threads=8)


def test_german_to_english(engine):
    lines = ["Die Lieferung erfolgt innerhalb von drei Werktagen."]
    [translated] = engine.translate(lines, "eng_Latn", "deu_Latn")
    assert "delivery" in translated.lower(), translated
    assert not translated.startswith("eng_Latn"), translated
    assert "Lieferung" in translated or "Delivery" in translated, translated


def test_src_lang_is_an_attribute_on_the_tokenizer(engine):
    engine.translate(["Hallo"], "eng_Latn", "deu_Latn")
    assert engine._tokenizer.src_lang == "deu_Latn"


def test_every_line_comes_back_in_order(engine):
    lines = ["Erster Satz.", "Zweiter Satz.", "Dritter Satz.", "Vierter Satz.", "Fünfter Satz."]
    translated = engine.translate(lines, "eng_Latn", "deu_Latn")
    assert len(translated) == len(lines)
    assert all(line.strip() for line in translated)


def test_the_claim_from_the_benchmark_is_reproducible(engine):
    # Literal quality, not DeepL: "Mit freundlichen Grüßen" is a known weak spot.
    [translated] = engine.translate(["Mit freundlichen Grüßen"], "eng_Latn", "deu_Latn")
    assert "greeting" in translated.lower() or "regards" in translated.lower(), translated


def test_chinese_script_resolution(engine):
    table = LanguageTable.load(TABLE)
    simplified = "这是一个简体中文的句子。"
    traditional = "這是一個繁體中文的句子。"
    [hans] = engine.translate([simplified], "eng_Latn", resolve_source(table, "zh", simplified))
    [hant] = engine.translate([traditional], "eng_Latn", resolve_source(table, "zh", traditional))
    assert hans.strip() and hant.strip()
    assert hans != hant
