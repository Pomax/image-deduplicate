#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use imgdedupe_core::matching::Thresholds;

mod app;
mod folder_picker;
mod fonts;
mod headless;
mod indexer;
mod settings;
mod thumbs;
#[cfg(debug_assertions)]
mod tools;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(debug_assertions, derive(clap::ValueEnum))]
pub enum Strictness {
    /// Only files that are the same picture pixel for pixel.
    Strict,
    /// Resizes, recompressions and format changes.
    Balanced,
    /// Also heavier edits, with more false pairs to reject by eye.
    Loose,
}

impl Strictness {
    pub fn thresholds(self, ignore_colour: bool) -> Thresholds {
        let mut thresholds = match self {
            Strictness::Strict => Thresholds::strict(),
            Strictness::Balanced => Thresholds::balanced(),
            Strictness::Loose => Thresholds::loose(),
        };
        thresholds.ignore_colour = ignore_colour;
        thresholds
    }

    /// A preset unless an explicit percentage was given, in which case that.
    #[cfg(debug_assertions)]
    fn resolve(self, sensitivity: Option<f64>, ignore_colour: bool) -> Thresholds {
        match sensitivity {
            Some(percent) => {
                let mut thresholds = Thresholds::at(percent);
                thresholds.ignore_colour = ignore_colour;
                thresholds
            }
            None => self.thresholds(ignore_colour),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Strictness::Strict => "strict",
            Strictness::Balanced => "balanced",
            Strictness::Loose => "loose",
        }
    }
}

fn main() -> ExitCode {
    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("imgdedupe: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// A development build takes a command line, so a report or a cleanup can be run
/// without the window. See `tools`.
#[cfg(debug_assertions)]
fn start() -> anyhow::Result<()> {
    tools::start()
}

/// A release build is the window, and the one flag that turns the log on.
#[cfg(not(debug_assertions))]
fn start() -> anyhow::Result<()> {
    if std::env::args().skip(1).any(|arg| arg == "--log") {
        imgdedupe_core::runlog::start("imgdedupe");
    }
    app::launch()
}

