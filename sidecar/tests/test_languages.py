import json
from dataclasses import replace
from pathlib import Path

import pytest

from nllb_sidecar.languages import (
    LanguageTable,
    LanguageTableError,
    looks_traditional,
    normalise_iso,
    resolve_source,
)

HERE = Path(__file__).resolve().parent
FIXTURE = HERE / "fixtures" / "nllb_language_tokens.txt"
TABLE_FILE = HERE.parent / "nllb_sidecar" / "data" / "nllb_codes.json"

# The picker in the application (src/lang.rs). A target the table cannot name
# would be a silent wrong-language translation, so it is asserted here too.
PICKER = [
    "en", "de", "fr", "es", "it", "pt", "nl", "pl", "ru", "uk", "tr", "cs",
    "sv", "da", "fi", "el", "hu", "ro", "ja", "zh", "ko", "ar", "id",
]


def vocabulary() -> set[str]:
    return {
        line.strip()
        for line in FIXTURE.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    }


@pytest.fixture(scope="module")
def table() -> LanguageTable:
    return LanguageTable.load(TABLE_FILE)


def test_the_fixture_is_the_real_model_vocabulary():
    tokens = vocabulary()
    assert len(tokens) == 202, "NLLB-200 has 202 language tokens"
    assert {"eng_Latn", "deu_Latn", "arb_Arab", "zho_Hans", "zho_Hant"} <= tokens


def test_every_code_exists_in_the_model_vocabulary(table):
    table.validate_against(vocabulary())


def test_every_picker_target_has_a_code(table):
    for iso in PICKER:
        code = table.code(iso)
        assert code, f"{iso} has no NLLB code"
        assert code in vocabulary(), f"{iso} -> {code} is not in the model vocabulary"


def test_the_tricky_targets_are_not_naive_iso639_3(table):
    # These are the codes a naive language-country guess gets wrong.
    assert table.code("ar") == "arb_Arab"
    assert table.code("zh") == "zho_Hans"
    assert table.code("ja") == "jpn_Jpan"
    assert table.code("ko") == "kor_Hang"
    assert table.code("el") == "ell_Grek"
    assert table.code("lv") == "lvs_Latn"
    assert table.code("sq") == "als_Latn"


def test_a_wrong_code_is_reported_not_ignored(table):
    broken = replace(table, languages={**table.languages, "de": "deu_Grek"})
    with pytest.raises(LanguageTableError) as error:
        broken.validate_against(vocabulary())
    assert "deu_Grek" in str(error.value)


def test_the_table_rejects_an_over_broad_code():
    assert LanguageTable.load(TABLE_FILE).code("xx") is None
    with pytest.raises(LanguageTableError):
        LanguageTable.load(TABLE_FILE).require("xx")


def test_normalise_handles_labels_regions_and_legacy_codes():
    assert normalise_iso("__label__de") == "de"
    assert normalise_iso("DE") == "de"
    assert normalise_iso("pt-BR") == "pt"
    assert normalise_iso("zh_Hant") == "zh"
    assert normalise_iso("iw") == "he"
    assert normalise_iso("in") == "id"
    assert normalise_iso("jw") == "jv"
    assert normalise_iso("no") == "nb"


def test_chinese_script_comes_from_the_text(table):
    simplified = "这是一个简体中文的句子"
    traditional = "這是一個繁體中文的句子"
    assert looks_traditional(traditional)
    assert not looks_traditional(simplified)
    assert resolve_source(table, "zh", simplified) == "zho_Hans"
    assert resolve_source(table, "zh", traditional) == "zho_Hant"
    # No Chinese characters at all: simplified is the documented default.
    assert resolve_source(table, "zh", "") == "zho_Hans"


def test_other_languages_ignore_the_text(table):
    assert resolve_source(table, "de", "這是一個繁體中文的句子") == "deu_Latn"
    assert resolve_source(table, "JA", "irrelevant") == "jpn_Jpan"


def test_the_table_file_is_what_the_application_reads():
    raw = json.loads(TABLE_FILE.read_text(encoding="utf-8"))
    assert raw["default_source"] in raw["languages"]
    assert 0.0 <= raw["min_confidence"] <= 1.0
    for iso, code in raw["languages"].items():
        assert iso == iso.lower(), iso
        assert code in vocabulary(), f"{iso} -> {code}"
