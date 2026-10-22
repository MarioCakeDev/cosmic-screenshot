//! Preview window: the captured region, a Translate button and the
//! source/target language pickers.
//!
//! The area selection itself belongs to the desktop portal (it is not part of
//! this tool), so the new button lives in this preview — the tool's own UI —
//! and acts on exactly the region the portal returned.

use std::fmt;
use std::path::PathBuf;

use iced::widget::{button, column, container, image, pick_list, row, text};
use iced::{Alignment, ContentFit, Element, Fill, Task, Theme};

use cosmic_screenshot_overlay::Canvas;
use cosmic_screenshot_translation::Lang;

use crate::lang::{self, LangInfo};
use crate::pipeline;

pub struct Request {
    pub image: PathBuf,
    pub initial: &'static LangInfo,
    /// Explicit source language, or `None` to let the engine detect it.
    pub initial_source: Option<Lang>,
    pub save_dir: Option<PathBuf>,
}

pub fn run(request: Request) -> Result<(), String> {
    let app = App::new(request)?;
    iced::application("Translate screenshot", App::update, App::view)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(1080.0, 720.0))
        .run_with(move || (app, Task::none()))
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Choice(&'static LangInfo);

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.0.name, self.0.code)
    }
}

/// The source picker: detection first, then every language the tool knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceChoice {
    Auto,
    Lang(&'static LangInfo),
}

impl fmt::Display for SourceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceChoice::Auto => f.write_str("Auto-detect"),
            SourceChoice::Lang(info) => write!(f, "{} ({})", info.name, info.code),
        }
    }
}

impl SourceChoice {
    fn lang(self) -> Option<Lang> {
        match self {
            SourceChoice::Auto => None,
            SourceChoice::Lang(info) => Some(lang::translation_lang(info)),
        }
    }
}

struct App {
    image_path: PathBuf,
    displayed: image::Handle,
    translated_path: Option<PathBuf>,
    choices: Vec<Choice>,
    sources: Vec<SourceChoice>,
    target: Choice,
    source: SourceChoice,
    status: String,
    busy: bool,
    save_dir: Option<PathBuf>,
}

impl App {
    fn new(request: Request) -> Result<Self, String> {
        let canvas = Canvas::open(&request.image).map_err(|error| error.to_string())?;
        let handle = image::Handle::from_rgba(canvas.width, canvas.height, canvas.rgba);
        let mut sources = Vec::with_capacity(lang::LANGS.len() + 1);
        sources.push(SourceChoice::Auto);
        sources.extend(lang::LANGS.iter().map(SourceChoice::Lang));
        let source = request
            .initial_source
            .and_then(|chosen| lang::find(chosen.code()))
            .map(SourceChoice::Lang)
            .unwrap_or(SourceChoice::Auto);
        Ok(Self {
            image_path: request.image,
            displayed: handle,
            translated_path: None,
            choices: lang::LANGS.iter().map(Choice).collect(),
            sources,
            target: Choice(request.initial),
            source,
            // The engine is checked by the button, so a stopped sidecar shows
            // its one-line message here rather than in a terminal.
            status: "Pick a language, then Translate.".to_string(),
            busy: false,
            save_dir: request.save_dir,
        })
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Target(choice) => {
                self.target = choice;
                Task::none()
            }
            Message::Source(choice) => {
                self.source = choice;
                Task::none()
            }
            Message::Translate => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Translating to {}…", self.target.0.name);
                let input = self.image_path.clone();
                let target = lang::translation_lang(self.target.0);
                let language = self.target.0;
                let source = self.source.lang();
                Task::perform(translate(input, target, language, source), Message::Done)
            }
            Message::Save => {
                self.save();
                Task::none()
            }
            Message::Close => iced::exit(),
            Message::Done(result) => {
                self.busy = false;
                match result {
                    Ok(done) => {
                        self.displayed = done.handle;
                        self.translated_path = Some(done.path.clone());
                        self.status = done.status;
                    }
                    Err(message) => self.status = message,
                }
                Task::none()
            }
        }
    }

    fn save(&mut self) {
        let source = self
            .translated_path
            .clone()
            .unwrap_or_else(|| self.image_path.clone());
        let Some(directory) = self.save_dir.clone().or_else(dirs::picture_dir) else {
            self.status = "could not locate the pictures directory; use --save-dir".to_string();
            return;
        };
        let suffix = if self.translated_path.is_some() {
            "translated"
        } else {
            "capture"
        };
        let timestamp = jiff::Zoned::now();
        let destination = directory.join(format!(
            "Screenshot_{}_{suffix}.png",
            timestamp.strftime("%Y-%m-%d_%H-%M-%S")
        ));
        match std::fs::copy(&source, &destination) {
            Ok(_) => self.status = format!("Saved {}", destination.display()),
            Err(error) => self.status = format!("could not save the image: {error}"),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let controls = row![
            text("From:"),
            pick_list(self.sources.as_slice(), Some(self.source), Message::Source),
            text("Translate to:"),
            pick_list(self.choices.as_slice(), Some(self.target), Message::Target),
            button("Translate").on_press_maybe((!self.busy).then_some(Message::Translate)),
            button("Save").on_press(Message::Save),
            button("Close").on_press(Message::Close),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        // The status gets its own line: an engine error is one long sentence and
        // must stay readable, not be squeezed out of the controls row.
        let header = column![controls, text(self.status.as_str())].spacing(8);

        column![
            container(header).padding(12).width(Fill),
            container(image(self.displayed.clone()).content_fit(ContentFit::Contain))
                .width(Fill)
                .height(Fill)
                .padding(12),
        ]
        .into()
    }
}

#[derive(Debug, Clone)]
enum Message {
    Target(Choice),
    Source(SourceChoice),
    Translate,
    Save,
    Close,
    Done(Result<Done, String>),
}

#[derive(Debug, Clone)]
struct Done {
    handle: image::Handle,
    path: PathBuf,
    status: String,
}

/// Blocking OCR + HTTP + render, run on iced's worker thread.
async fn translate(
    input: PathBuf,
    target: Lang,
    language: &'static LangInfo,
    source: Option<Lang>,
) -> Result<Done, String> {
    let translator = pipeline::translator_from_env().map_err(|error| error.to_string())?;
    let mut job = pipeline::Job::new(&input, target);
    job.source = source;
    let outcome = pipeline::run(&job, translator.as_ref()).map_err(|error| error.to_string())?;
    let canvas = Canvas::open(&outcome.output).map_err(|error| error.to_string())?;
    let handle = image::Handle::from_rgba(canvas.width, canvas.height, canvas.rgba);
    let source = outcome
        .detected_source
        .map(|source| source.english_name().to_string())
        .unwrap_or_else(|| "detected language".to_string());
    let status = format!(
        "{} → {} · {} line(s) · saved to {}",
        source,
        language.name,
        outcome.report.drawn,
        outcome.output.display()
    );
    Ok(Done {
        handle,
        path: outcome.output,
        status,
    })
}
