//! The NLLB sidecar client over the real HTTP stack, against a throwaway local
//! server: batching, the wire format, and every failure the client has to turn
//! into one line.

mod support;

use cosmic_screenshot_translation::{Lang, LocalTranslator, Provider, TranslateError, Translator};
use support::{http_response, serve};

fn translations_body(items: &[&str], source: &str) -> String {
    serde_json::json!({
        "translations": items,
        "source": source,
        "source_confidence": 0.98,
        "detected": true,
        "unchanged": false,
    })
    .to_string()
}

#[test]
fn every_line_goes_in_one_request_with_canonical_codes() {
    let body = translations_body(&["Hello", "World"], "de");
    let (url, requests) = serve(vec![http_response("200 OK", &body)]);
    let translator = LocalTranslator::new(url);

    let batch = translator
        .translate(&["Hallo", "Welt"], Lang::parse("en").unwrap(), None)
        .expect("translation succeeds");

    assert_eq!(batch.items[0].text, "Hello");
    assert_eq!(batch.items[1].text, "World");
    assert_eq!(batch.detected_source().map(Lang::code), Some("de"));
    assert_eq!(translator.provider(), Provider::LocalNmt);

    let request = requests.recv().expect("one request");
    assert!(request.starts_with("POST /translate HTTP/1.1"), "{request}");
    assert!(request.contains(r#""lines":["Hallo","Welt"]"#), "{request}");
    assert!(request.contains(r#""target":"en""#), "{request}");
    assert!(request.contains(r#""source":"auto""#), "{request}");
    assert!(
        requests.try_recv().is_err(),
        "one capture must be one request"
    );
}

#[test]
fn fifty_one_lines_still_go_in_one_request() {
    // NLLB has a fixed per-call overhead, so the client never chunks a capture.
    let items: Vec<String> = (0..51).map(|i| format!("t{i}")).collect();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let (url, requests) = serve(vec![http_response(
        "200 OK",
        &translations_body(&refs, "en"),
    )]);
    let translator = LocalTranslator::new(url);
    let texts: Vec<String> = (0..51).map(|i| format!("line {i}")).collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();

    let batch = translator
        .translate(&texts, Lang::parse("de").unwrap(), None)
        .expect("translation succeeds");
    assert_eq!(batch.items.len(), 51);

    let request = requests.recv().expect("one request");
    assert_eq!(
        request.matches(r#""line "#).count(),
        51,
        "all lines belong to the single request"
    );
}

#[test]
fn an_explicit_source_is_sent_as_a_code() {
    let (url, requests) = serve(vec![http_response(
        "200 OK",
        &translations_body(&["Hello"], "de"),
    )]);
    let translator = LocalTranslator::new(url);
    translator
        .translate(
            &["Hallo"],
            Lang::parse("en").unwrap(),
            Some(Lang::parse("de").unwrap()),
        )
        .expect("translation succeeds");
    let request = requests.recv().expect("one request");
    assert!(request.contains(r#""source":"de""#), "{request}");
}

#[test]
fn an_unreachable_sidecar_is_one_clear_line() {
    // Port 9 on loopback is closed; the connection is refused immediately.
    let translator = LocalTranslator::new("http://127.0.0.1:9");
    let error = translator
        .translate(&["Hallo"], Lang::parse("en").unwrap(), None)
        .unwrap_err();
    assert!(
        matches!(error, TranslateError::Unavailable { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(
        message.starts_with(
            "translation engine not available: cannot reach the local NLLB sidecar at http://127.0.0.1:9"
        ),
        "{message}"
    );
    assert!(message.contains("systemctl --user start cosmic-screenshot-nllb"));
    assert!(!message.contains('\n'), "{message}");
}

#[test]
fn a_missing_model_is_one_clear_line() {
    let (url, _requests) = serve(vec![http_response(
        "503 Service Unavailable",
        r#"{"error":"NLLB model not found at /models/nllb","code":"engine_failed"}"#,
    )]);
    let translator = LocalTranslator::new(url);
    let error = translator
        .translate(&["Hallo"], Lang::parse("en").unwrap(), None)
        .unwrap_err();
    assert!(
        matches!(error, TranslateError::Unavailable { .. }),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "translation engine not available: NLLB model not found at /models/nllb"
    );
}

#[test]
fn a_rejected_target_is_one_clear_line() {
    let (url, _requests) = serve(vec![http_response(
        "400 Bad Request",
        r#"{"error":"unknown target language 'tlh'","code":"unknown_language","supported":["de","en"]}"#,
    )]);
    let translator = LocalTranslator::new(url);
    let error = translator
        .translate(&["Hallo"], Lang::parse("en").unwrap(), None)
        .unwrap_err();
    assert!(matches!(
        error,
        TranslateError::Rejected { status: 400, .. }
    ));
    assert_eq!(
        error.to_string(),
        "translation engine rejected the request (400): unknown target language 'tlh'"
    );
}

#[test]
fn a_short_response_is_a_count_mismatch() {
    let (url, _requests) = serve(vec![http_response(
        "200 OK",
        &translations_body(&["only"], "de"),
    )]);
    let translator = LocalTranslator::new(url);
    let error = translator
        .translate(&["eins", "zwei"], Lang::parse("en").unwrap(), None)
        .unwrap_err();
    assert!(
        error.to_string().contains("1 translations for 2"),
        "{error}"
    );
}

#[test]
fn the_health_check_reports_a_ready_sidecar() {
    let (url, requests) = serve(vec![http_response(
        "200 OK",
        r#"{"ok":true,"device":"cpu","compute_type":"int8","languages":109}"#,
    )]);
    LocalTranslator::new(url).check().expect("sidecar is ready");
    let request = requests.recv().expect("one request");
    assert!(request.starts_with("GET /health HTTP/1.1"), "{request}");
}

#[test]
fn the_health_check_carries_the_model_missing_reason() {
    let (url, _requests) = serve(vec![http_response(
        "503 Service Unavailable",
        r#"{"error":"NLLB model not found at /models/nllb","code":"engine_failed"}"#,
    )]);
    let error = LocalTranslator::new(url).check().unwrap_err();
    assert_eq!(
        error.to_string(),
        "translation engine not available: NLLB model not found at /models/nllb"
    );
}

#[test]
fn the_health_check_reports_a_stopped_sidecar() {
    let error = LocalTranslator::new("http://127.0.0.1:9")
        .check()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("translation engine not available: cannot reach the local NLLB sidecar"),
        "{error}"
    );
}

#[test]
fn a_malformed_url_is_configuration_not_a_panic() {
    // Exercised through the parser, not the network.
    let error = LocalTranslator::new("not-a-url")
        .translate(&["Hallo"], Lang::parse("en").unwrap(), None)
        .unwrap_err();
    assert!(
        matches!(error, TranslateError::Unavailable { .. }),
        "{error}"
    );
}
