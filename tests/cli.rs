//! The command-line contract the owner asked for: `--translate` without a key
//! fails with one clear line and a non-zero exit code, and never captures.

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

#[test]
fn translate_without_a_key_fails_with_one_clear_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = Command::new(binary())
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--no-gui",
        ])
        .env_remove("DEEPL_API_KEY")
        .env_remove("DEEPL_HOST")
        .env_remove("TRANSLATE_PROVIDER")
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "translation engine not configured: set DEEPL_API_KEY"
    );
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
fn an_unknown_language_lists_the_supported_ones() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = Command::new(binary())
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "klingon",
            "--no-gui",
        ])
        .env_remove("DEEPL_API_KEY")
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
fn an_unknown_provider_is_one_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let image = sample_image(directory.path());

    let output = Command::new(binary())
        .args([
            "--input",
            image.to_str().unwrap(),
            "--translate",
            "de",
            "--no-gui",
        ])
        .env("DEEPL_API_KEY", "test-key")
        .env("TRANSLATE_PROVIDER", "google")
        .output()
        .expect("run the binary");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("translation provider 'google' is not implemented yet"),
        "{stderr}"
    );
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
        "--ocr-langs",
    ] {
        assert!(
            stdout.contains(flag),
            "{flag} missing from --help:\n{stdout}"
        );
    }
}
