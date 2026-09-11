use std::path::{Path, PathBuf};

const FILE: &str = "settings.conf";

/// Where the operating system keeps an application's configuration: `%APPDATA%`
/// on Windows, `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or
/// `~/.config` elsewhere. Not beside the executable, which may sit somewhere the
/// user cannot write to.
fn settings_path() -> PathBuf {
    match directories::ProjectDirs::from("", "", "imgdedupe") {
        Some(dirs) => {
            let dir = dirs.config_dir().to_path_buf();
            let _ = std::fs::create_dir_all(&dir);
            dir.join(FILE)
        }
        None => PathBuf::from(FILE),
    }
}

/// How the application was left last time.
///
/// This is the application's own state and not a fact about any folder, so it
/// lives here and not in the index. An index describes the files it was built
/// from; how someone was driving the window is not one of those.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Whatever was chosen, kept exactly as it was written.
    ///
    /// It is never checked for existence on load. A network share that is slow to
    /// answer, or offline for a minute, makes that check say no, and dropping the
    /// setting on the strength of it loses the choice for good on the next save.
    /// If the folder cannot be read, the scan says so.
    pub folder: Option<PathBuf>,
    pub recurse: bool,
    /// Folders that have been scanned, in alphabetical order. Opening a folder
    /// does not put one here: scanning it does.
    pub previous: Vec<PathBuf>,
    pub ignore_colour: bool,
    /// Where the window was and how big, or nothing the first time it is opened.
    pub window: Option<Window>,
    /// Width of the preview pane in the review view, so the divider comes back
    /// where it was left.
    pub preview_width: Option<f32>,
}

/// The window's outer position and inner size, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            folder: None,
            recurse: false,
            previous: Vec::new(),
            ignore_colour: false,
            window: None,
            preview_width: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        let loaded = read(&path);
        imgdedupe_core::log_line!(
            "settings {}: exists {}, {}",
            path.display(),
            path.exists(),
            loaded.describe()
        );
        loaded
    }

    /// Under test this writes nothing. The suite drives the window for real, and
    /// the file it would write to is this machine's own configuration.
    #[cfg(test)]
    pub fn save(&self) {}

    #[cfg(not(test))]
    pub fn save(&self) {
        let path = settings_path();
        write(&path, self);
        imgdedupe_core::log_line!("saved settings to {}: {}", path.display(), self.describe());
    }

    /// What the settings say, for the run log, which is the only thing that
    /// reads it.
    #[cfg(feature = "logging")]
    fn describe(&self) -> String {
        format!(
            "folder {:?}, recurse {}, {} scanned before, ignore_colour {}, \
             window {:?}, preview {:?}",
            self.folder,
            self.recurse,
            self.previous.len(),
            self.ignore_colour,
            self.window,
            self.preview_width
        )
    }
}

/// The folders in the order they are offered: alphabetical, letter case ignored,
/// and each one once.
pub fn sorted(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if !out.contains(folder) {
            out.push(folder.clone());
        }
    }
    out.sort_by_key(|folder| folder.display().to_string().to_lowercase());
    out
}

fn read(settings: &Path) -> Settings {
    let mut out = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings) else {
        return out;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "folder" => {
                let folder = value.trim();
                out.folder = (!folder.is_empty()).then(|| PathBuf::from(folder));
            }
            "recurse" => out.recurse = value.trim() == "1",
            "previous" => {
                let folder = value.trim();
                if !folder.is_empty() {
                    out.previous.push(PathBuf::from(folder));
                }
            }
            "ignore_colour" => out.ignore_colour = value.trim() == "1",
            "window" => out.window = parse_window(value.trim()),
            "preview_width" => {
                out.preview_width = value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|width| *width > 0.0);
            }
            _ => {}
        }
    }
    out.previous = sorted(&out.previous);
    out
}

/// `x,y,width,height,maximized`. A line that is not all five numbers is no
/// window rather than a broken one.
fn parse_window(value: &str) -> Option<Window> {
    let mut parts = value.split(',').map(str::trim);
    let mut number = || parts.next()?.parse::<f32>().ok();
    let (x, y, width, height) = (number()?, number()?, number()?, number()?);
    let maximized = parts.next() == Some("1");
    if !(width > 0.0 && height > 0.0) {
        return None;
    }
    Some(Window {
        x,
        y,
        width,
        height,
        maximized,
    })
}

fn write(settings: &Path, values: &Settings) {
    let folder = values
        .folder
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let flag = |on: bool| if on { "1" } else { "0" };
    let mut text = format!(
        "folder={folder}\nrecurse={}\nignore_colour={}\n",
        flag(values.recurse),
        flag(values.ignore_colour)
    );
    // One line each, written in the order they will be shown in.
    for folder in sorted(&values.previous) {
        text.push_str(&format!("previous={}\n", folder.display()));
    }
    if let Some(window) = values.window {
        text.push_str(&format!(
            "window={},{},{},{},{}\n",
            window.x,
            window.y,
            window.width,
            window.height,
            flag(window.maximized)
        ));
    }
    if let Some(width) = values.preview_width {
        text.push_str(&format!("preview_width={width}\n"));
    }
    let _ = std::fs::write(settings, text);
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
