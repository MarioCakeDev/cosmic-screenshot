//! The application's 23 target languages against the sidecar's NLLB table and
//! the model's real language list.
//!
//! The fixture is the 202 language tokens extracted from
//! `facebook/nllb-200-distilled-600M/tokenizer.json`; the sidecar checks the
//! same table against the loaded tokenizer at startup. This test catches the
//! failure that would otherwise be silent: a picker entry whose NLLB code does
//! not exist, or names a different language than the one shown.

use std::collections::BTreeMap;

use cosmic_screenshot::lang::LANGS;
use cosmic_screenshot_overlay::Script;

const TABLE_JSON: &str = include_str!("../sidecar/nllb_sidecar/data/nllb_codes.json");
const MODEL_TOKENS: &str = include_str!("../sidecar/tests/fixtures/nllb_language_tokens.txt");

fn model_tokens() -> Vec<&'static str> {
    MODEL_TOKENS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn table() -> BTreeMap<String, String> {
    let value: serde_json::Value = serde_json::from_str(TABLE_JSON).expect("table is JSON");
    let languages = value["languages"].as_object().expect("languages object");
    languages
        .iter()
        .map(|(iso, code)| {
            (
                iso.clone(),
                code.as_str().expect("code is a string").to_string(),
            )
        })
        .collect()
}

/// The codes the model actually uses. A naive ISO 639-3 guess gets several of
/// these wrong (`ar` is `arb_Arab`, not `ara_Arab`; `lv` is `lvs_Latn`).
const EXPECTED: &[(&str, &str)] = &[
    ("en", "eng_Latn"),
    ("de", "deu_Latn"),
    ("fr", "fra_Latn"),
    ("es", "spa_Latn"),
    ("it", "ita_Latn"),
    ("pt", "por_Latn"),
    ("nl", "nld_Latn"),
    ("pl", "pol_Latn"),
    ("ru", "rus_Cyrl"),
    ("uk", "ukr_Cyrl"),
    ("tr", "tur_Latn"),
    ("cs", "ces_Latn"),
    ("sv", "swe_Latn"),
    ("da", "dan_Latn"),
    ("fi", "fin_Latn"),
    ("el", "ell_Grek"),
    ("hu", "hun_Latn"),
    ("ro", "ron_Latn"),
    ("ja", "jpn_Jpan"),
    ("zh", "zho_Hans"),
    ("ko", "kor_Hang"),
    ("ar", "arb_Arab"),
    ("id", "ind_Latn"),
];

#[test]
fn every_picker_language_has_a_code_in_the_model() {
    let table = table();
    let tokens = model_tokens();
    assert_eq!(
        tokens.len(),
        202,
        "the fixture is the model's language list"
    );
    for info in LANGS {
        let code = table
            .get(info.code)
            .unwrap_or_else(|| panic!("no NLLB code for picker language {}", info.code));
        assert!(
            tokens.contains(&code.as_str()),
            "{} -> {code} is not in the model vocabulary",
            info.code
        );
    }
}

#[test]
fn the_codes_are_the_documented_ones() {
    let table = table();
    for (iso, expected) in EXPECTED {
        assert_eq!(
            table.get(*iso).map(String::as_str),
            Some(*expected),
            "{iso} maps to the wrong NLLB language"
        );
    }
}

#[test]
fn every_code_has_a_script_that_matches_the_overlay() {
    let table = table();
    for info in LANGS {
        let code = table.get(info.code).expect("picker language has a code");
        let script = code.split_once('_').map(|(_, script)| script).unwrap_or("");
        let ok = match info.script {
            Script::Latin => script == "Latn",
            Script::Cyrillic => script == "Cyrl",
            Script::Greek => script == "Grek",
            Script::Arabic => script == "Arab",
            Script::Korean => script == "Hang",
            // CJK covers Japanese, simplified and traditional Chinese.
            Script::Cjk => matches!(script, "Jpan" | "Hans" | "Hant"),
        };
        assert!(
            ok,
            "{} -> {code} has script {script} for {:?}",
            info.code, info.script
        );
    }
}

#[test]
fn the_table_default_source_is_a_picker_language() {
    let value: serde_json::Value = serde_json::from_str(TABLE_JSON).expect("table is JSON");
    let default = value["default_source"].as_str().expect("default_source");
    assert!(
        LANGS.iter().any(|info| info.code == default),
        "the sidecar falls back to {default}, which the picker cannot show"
    );
}
