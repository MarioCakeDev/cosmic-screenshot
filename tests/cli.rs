//! The command-line contract: failures are one clear line and a non-zero exit
//! code, and never cost the user a capture or write an output file.

use std::process::Command;

use cosmic_screenshot_overlay::Canvas;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_cosmic-screenshot-translate")
}

fn sample_image(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("shot.png");
    Canvas::filled(8, 8, [255, 255, 255, 255])
        .save(&path)
        .expect("write sample png");
    path
}

/// A command with the engine environment cleared, so the host's own settings
/// cannot make a test pass (or fail) by accident.
fn command() -> Command {
    let mut command = Command::new(binary());
    for variable in [
        "TRANSLATE_PROVIDER",
        "DEEPL_API_KEY",
        "DEEPL_HOST",
        "COSMIC_SCREENSHOT_NLLB_URL",
    ] {
        command.env_remove(variable);
    }
    command
}

#[test]
fn the_default_engine_without_a_sidecar_fails_with_one_clear_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--no-gui",
        ])
        .env("COSMIC_SCREENSHOT_NLLB_URL", "http://127.0.0.1:9")
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("translation engine not available: cannot reach the local NLLB sidecar at http://127.0.0.1:9"),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "nothing may be printed as a result"
    );
    assert!(
        !image.with_extension("translated.png").exists(),
        "no output may be written without an engine"
    );
}

#[test]
fn deepl_without_a_key_fails_with_one_clear_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--no-gui",
        ])
        .env("TRANSLATE_PROVIDER", "deepl")
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "translation engine not configured: set DEEPL_API_KEY"
    );
    assert!(
        !image.with_extension("translated.png").exists(),
        "no output may be written without an engine"
    );
}

#[test]
fn an_unknown_language_lists_the_supported_ones() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "klingon",
            "--no-gui",
        ])
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported target language 'klingon'"),
        "{stderr}"
    );
    assert!(stderr.contains("de (German)"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
}

#[test]
fn an_unknown_source_language_is_rejected_before_the_capture() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--source-lang",
            "klingon",
            "--no-gui",
        ])
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported source language 'klingon'"),
        "{stderr}"
    );
    assert!(stderr.contains("use 'auto'"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
}

#[test]
fn an_unknown_preview_target_is_rejected_before_the_capture() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--target-language",
            "klingon",
            "--no-gui",
        ])
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported target language 'klingon'"),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "the capture path must not be printed before an invalid target is rejected"
    );
}

#[test]
fn an_unknown_provider_is_one_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = command()
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--no-gui",
        ])
        .env("TRANSLATE_PROVIDER", "google")
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("translation provider 'google' is not implemented yet"),
        "{stderr}"
    );
    assert!(stderr.contains("local-nmt"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
}

#[test]
fn help_documents_the_new_flags() {
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("run the binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--translate",
        "--input",
        "--output",
        "--target-language",
        "--source-lang",
        "--ocr-langs",
    ] {
        assert!(
            stdout.contains(flag),
            "{flag} missing from --help:\n{stdout}"
        );
    }
}
