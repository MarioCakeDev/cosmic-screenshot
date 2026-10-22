//! `cosmic-screenshot-translate`
//!
//! Captures a region through the XDG desktop portal (the stock selection UI),
//! then either translates it immediately (`--translate <language>`) or opens a
//! preview window with a Translate button and a target-language picker.

use ashpd::desktop::screenshot::Screenshot;
use clap::{ArgAction, Parser};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::ExitCode;
use zbus::{Connection, proxy, zvariant::Value};

use cosmic_screenshot::lang::{self, LangInfo};
use cosmic_screenshot::pipeline;

mod localize;

#[derive(Parser, Default, Debug, Clone, PartialEq, Eq)]
#[command(
    version,
    about = "Capture a screenshot and translate the captured text in place"
)]
struct Args {
    /// Enable interactive mode in the portal
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set)]
    interactive: bool,
    /// Enable modal mode in the portal
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set,)]
    modal: bool,
    /// Send a notification with the path to the saved screenshot
    #[clap(long,
        default_missing_value("true"),
        default_value("true"),
        num_args(0..=1),
        require_equals(true),
        action = ArgAction::Set)]
    notify: bool,
    /// The directory to save the screenshot to, if not performing an interactive screenshot
    #[clap(short, long)]
    save_dir: Option<PathBuf>,
    /// Translate the captured area into LANGUAGE (e.g. de, ja) and save the
    /// result, without opening the preview window
    #[arg(long, value_name = "LANGUAGE")]
    translate: Option<String>,
    /// Translate an existing image instead of capturing one
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Where to write the translated image (default: <capture>.translated.png)
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,
    /// Target language preselected in the preview window
    #[arg(long, value_name = "LANGUAGE")]
    target_language: Option<String>,
    /// Source language of the captured text (default: detect it)
    #[arg(long, value_name = "LANGUAGE")]
    source_lang: Option<String>,
    /// Never open the preview window
    #[arg(long)]
    no_gui: bool,
    /// Tesseract -l value for the source languages (e.g. eng+deu)
    #[arg(long, value_name = "LIST")]
    ocr_langs: Option<String>,
}

#[proxy(assume_defaults = true)]
trait Notifications {
    /// Call the org.freedesktop.Notifications.Notify D-Bus method
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, &Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

enum Capture {
    File(PathBuf),
    Clipboard,
    Cancelled,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    crate::localize::localize();
    let args = Args::parse();

    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        // Every failure path prints exactly one line, never a panic.
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    // Resolve the target, the source override and the engine *before* taking a
    // screenshot: an unavailable engine must not cost the user a capture.
    let source = match &args.source_lang {
        Some(requested) if requested.trim().eq_ignore_ascii_case("auto") => None,
        Some(requested) => Some(resolve_source_language(requested)?),
        None => None,
    };
    let translation = match &args.translate {
        Some(requested) => {
            let info = resolve_language(requested)?;
            let translator = pipeline::translator_from_env().map_err(|error| error.to_string())?;
            Some((info, translator))
        }
        None => None,
    };

    let capture = match &args.input {
        Some(path) => Capture::File(path.clone()),
        None => capture(&args).await?,
    };

    let path = match capture {
        Capture::Cancelled => {
            println!("Screenshot cancelled by user");
            return Ok(());
        }
        Capture::Clipboard => {
            if translation.is_some() {
                return Err(
                    "cannot translate a clipboard-only capture; save the screenshot to a file first"
                        .to_string(),
                );
            }
            println!();
            return Ok(());
        }
        Capture::File(path) => path,
    };

    if let Some((info, translator)) = translation {
        let mut job = pipeline::Job::new(&path, lang::translation_lang(info));
        job.source = source;
        job.ocr = pipeline::ocr_config(args.ocr_langs.clone());
        if let Some(output) = &args.output {
            job.output = output.clone();
        }
        let outcome = pipeline::run(&job, translator.as_ref()).map_err(|e| e.to_string())?;
        println!("{}", outcome.output.display());
        if args.notify {
            let source = outcome
                .detected_source
                .map(|lang| lang.english_name().to_string())
                .unwrap_or_else(|| "the source language".to_string());
            let body = format!(
                "{} → {} · {} line(s) · {}",
                source,
                info.name,
                outcome.report.drawn,
                outcome.output.display()
            );
            notify(&body).await;
        }
        return Ok(());
    }

    println!("{}", path.display());

    #[cfg(feature = "gui")]
    if !args.no_gui && args.interactive {
        let initial = match &args.target_language {
            Some(requested) => resolve_language(requested)?,
            None => lang::from_locale()
                .unwrap_or_else(|| lang::find("en").expect("the language table contains English")),
        };
        let request = cosmic_screenshot::gui::Request {
            image: path.clone(),
            initial,
            initial_source: source,
            save_dir: args.save_dir.clone(),
        };
        cosmic_screenshot::gui::run(request)
            .map_err(|error| format!("could not open the preview window: {error}"))?;
        return Ok(());
    }

    if args.notify {
        notify(&path.display().to_string()).await;
    }
    Ok(())
}

fn resolve_language(requested: &str) -> Result<&'static LangInfo, String> {
    lang::from_argument(requested).ok_or_else(|| {
        format!(
            "unsupported target language '{requested}'; supported: {}",
            lang::supported_list()
        )
    })
}

fn resolve_source_language(requested: &str) -> Result<cosmic_screenshot_translation::Lang, String> {
    lang::from_argument(requested)
        .map(lang::translation_lang)
        .ok_or_else(|| {
            format!(
                "unsupported source language '{requested}'; use 'auto' or one of: {}",
                lang::supported_list()
            )
        })
}

async fn capture(args: &Args) -> Result<Capture, String> {
    let picture_dir = args
        .save_dir
        .clone()
        .filter(|dir| dir.is_dir())
        .or_else(|| (!args.interactive).then(dirs::picture_dir).flatten());

    let response = Screenshot::request()
        .interactive(args.interactive)
        .modal(args.modal)
        .send()
        .await
        .map_err(|error| format!("failed to send screenshot request: {error}"))?
        .response();

    let response = match response {
        Err(error) => {
            if error.to_string().contains("Cancelled") {
                return Ok(Capture::Cancelled);
            }
            return Err(format!("error taking screenshot: {error}"));
        }
        Ok(response) => response,
    };

    let uri = response.uri();
    match uri.scheme() {
        "file" => {
            let response_path = uri
                .to_file_path()
                .map_err(|_| format!("unsupported response URI '{uri}'"))?;
            let path = match picture_dir {
                Some(picture_dir) => {
                    let date = jiff::Zoned::now();
                    let filename = format!("Screenshot_{}.png", date.strftime("%Y-%m-%d_%H-%M-%S"));
                    let path = picture_dir.join(filename);
                    let same_device = std::fs::metadata(&picture_dir)
                        .and_then(|metadata| {
                            std::fs::metadata(&response_path)
                                .map(|temporary| metadata.dev() == temporary.dev())
                        })
                        .unwrap_or(false);
                    if same_device {
                        std::fs::rename(&response_path, &path)
                            .map_err(|error| format!("failed to move screenshot: {error}"))?;
                    } else {
                        std::fs::copy(&response_path, &path)
                            .map_err(|error| format!("failed to copy screenshot: {error}"))?;
                        _ = std::fs::remove_file(&response_path);
                    }
                    path
                }
                None => response_path,
            };
            Ok(Capture::File(path))
        }
        "clipboard" => Ok(Capture::Clipboard),
        scheme => Err(format!("unsupported scheme '{scheme}'")),
    }
}

async fn notify(body: &str) {
    let Ok(connection) = Connection::session().await else {
        return;
    };
    let Ok(proxy) = NotificationsProxy::new(&connection).await else {
        return;
    };
    _ = proxy
        .notify(
            &fl!("cosmic-screenshot"),
            0,
            "com.system76.CosmicScreenshot",
            &fl!("cosmic-screenshot"),
            body,
            &[],
            HashMap::from([("transient", &Value::Bool(true))]),
            5000,
        )
        .await;
}
