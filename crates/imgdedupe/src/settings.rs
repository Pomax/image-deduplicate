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
    /// Whether that folder keeps an index. The folder itself comes back either
    /// way; this decides whether there is anything to open it with.
    pub remember_folder: bool,
    pub recurse: bool,
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
            remember_folder: false,
            recurse: false,
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
        imgdedupe_core::runlog::line(&format!(
            "settings {}: exists {}, {}",
            path.display(),
            path.exists(),
            loaded.describe()
        ));
        loaded
    }

    pub fn save(&self) {
        let path = settings_path();
        write(&path, self);
        imgdedupe_core::runlog::line(&format!(
            "saved settings to {}: {}",
            path.display(),
            self.describe()
        ));
    }

    fn describe(&self) -> String {
        format!(
            "folder {:?} (remembered {}), recurse {}, ignore_colour {}, \
             window {:?}, preview {:?}",
            self.folder,
            self.remember_folder,
            self.recurse,
            self.ignore_colour,
            self.window,
            self.preview_width
        )
    }
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
            "remember_folder" => out.remember_folder = value.trim() == "1",
            "recurse" => out.recurse = value.trim() == "1",
            "ignore_colour" => out.ignore_colour = value.trim() == "1",
            "window" => out.window = parse_window(value.trim()),
            "preview_width" => {
                out.preview_width = value.trim().parse::<f32>().ok().filter(|width| *width > 0.0);
            }
            _ => {}
        }
    }
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
    Some(Window { x, y, width, height, maximized })
}

fn write(settings: &Path, values: &Settings) {
    let folder = values
        .folder
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let flag = |on: bool| if on { "1" } else { "0" };
    let mut text = format!(
        "folder={folder}\nremember_folder={}\nrecurse={}\nignore_colour={}\n",
        flag(values.remember_folder),
        flag(values.recurse),
        flag(values.ignore_colour)
    );
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
mod tests {
    use super::*;

    fn folder_in(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir(&path).expect("mkdir");
        path
    }

    #[test]
    fn the_folder_and_the_recurse_flag_both_come_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        let chosen = folder_in(dir.path(), "pictures");

        // The folder comes back either way. What the tick decides is whether it
        // has an index to open with.
        let saved = Settings { folder: Some(chosen.clone()), recurse: false, ..Settings::default() };
        write(&settings, &saved);
        assert_eq!(read(&settings), saved);

        let saved = Settings { folder: Some(chosen), remember_folder: true, recurse: true, ..Settings::default() };
        write(&settings, &saved);
        assert_eq!(read(&settings), saved);
    }

    #[test]
    fn a_folder_that_cannot_be_reached_right_now_is_still_remembered() {
        // A network share that is asleep, offline for a minute, or slow to answer
        // must not cost the setting. Checking it exists on load and dropping it
        // when the check says no threw the choice away for good.
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        let chosen = folder_in(dir.path(), "pictures");

        write(
            &settings,
            &Settings {
                folder: Some(chosen.clone()),
                remember_folder: true,
                ..Settings::default()
            },
        );
        std::fs::remove_dir(&chosen).expect("remove");

        let loaded = read(&settings);
        assert_eq!(loaded.folder, Some(chosen));
        assert!(!loaded.recurse);
    }

    #[test]
    fn a_unc_path_survives_the_round_trip_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        let share = PathBuf::from(r"\\DragonHoard\Storage\Seafood\sexy pictures\gonewild");

        write(
            &settings,
            &Settings {
                folder: Some(share.clone()),
                remember_folder: true,
                ..Settings::default()
            },
        );
        assert_eq!(read(&settings).folder, Some(share));
    }

    #[test]
    fn the_window_place_comes_back_as_it_was_left() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);

        for place in [
            Window { x: 120.0, y: 64.0, width: 1400.0, height: 900.0, maximized: false },
            Window { x: -1920.0, y: 0.0, width: 800.0, height: 600.0, maximized: true },
        ] {
            write(&settings, &Settings { window: Some(place), ..Settings::default() });
            assert_eq!(read(&settings).window, Some(place));
        }
    }

    #[test]
    fn the_divider_between_the_list_and_the_preview_comes_back_where_it_was() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);

        write(&settings, &Settings { preview_width: Some(612.5), ..Settings::default() });
        assert_eq!(read(&settings).preview_width, Some(612.5));

        for damaged in ["preview_width=\n", "preview_width=wide\n", "preview_width=0\n"] {
            std::fs::write(&settings, damaged).expect("write");
            assert_eq!(read(&settings).preview_width, None, "on {damaged:?}");
        }
    }

    #[test]
    fn no_window_line_means_no_remembered_place_rather_than_a_broken_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);

        write(&settings, &Settings::default());
        assert_eq!(read(&settings).window, None);

        for damaged in [
            "window=\n",
            "window=1,2\n",
            "window=a,b,c,d,e\n",
            "window=0,0,0,0,0\n",
            "window=10,10,-4,300,0\n",
        ] {
            std::fs::write(&settings, damaged).expect("write");
            assert_eq!(read(&settings).window, None, "on {damaged:?}");
        }
    }

    /// A file written by an older version still has a sensitivity line in it.
    /// It is ignored, so the slider starts where it starts.
    #[test]
    fn a_sensitivity_line_left_by_an_older_version_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        std::fs::write(&settings, "folder=\nsensitivity=50\nignore_colour=1\n").expect("write");
        assert_eq!(read(&settings), Settings { ignore_colour: true, ..Settings::default() });
    }

    #[test]
    fn no_settings_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(&dir.path().join("absent")), Settings::default());
    }

    #[test]
    fn a_damaged_file_gives_the_defaults_rather_than_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        for content in ["", "   \n", "nonsense", "folder\nrecurse", "=\n=\n"] {
            std::fs::write(&settings, content).expect("write");
            assert_eq!(read(&settings), Settings::default(), "on {content:?}");
        }
    }

    #[test]
    fn a_path_with_spaces_survives_the_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        let chosen = folder_in(dir.path(), "sexy pictures");

        write(&settings, &Settings { folder: Some(chosen.clone()), remember_folder: true, recurse: true, ..Settings::default() });
        assert_eq!(read(&settings).folder, Some(chosen));
    }

    #[test]
    fn the_path_is_stored_exactly_as_it_was_given() {
        // Nothing normalises, resolves or rewrites it on the way in or out.
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        for path in [
            r"\\server\share\folder",
            r"C:\Users\Mike\Pictures",
            r"D:\link-to-somewhere",
            "/mnt/photos",
        ] {
            let given = PathBuf::from(path);
            write(&settings, &Settings { folder: Some(given.clone()), remember_folder: true, recurse: true, ..Settings::default() });
            assert_eq!(read(&settings).folder, Some(given), "on {path}");
        }
    }

    #[test]
    fn saving_again_replaces_what_was_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = dir.path().join(FILE);
        let first = folder_in(dir.path(), "one");
        let second = folder_in(dir.path(), "two");

        write(&settings, &Settings { folder: Some(first), remember_folder: true, recurse: true, ..Settings::default() });
        write(
            &settings,
            &Settings {
                folder: Some(second.clone()),
                remember_folder: true,
                ..Settings::default()
            },
        );

        let loaded = read(&settings);
        assert_eq!(loaded.folder, Some(second));
        assert!(!loaded.recurse);
    }

    #[test]
    fn the_index_is_not_asked_about_any_of_this() {
        // These are the application's settings, not facts about a folder, so
        // nothing here goes near a database.
        let default = Settings::default();
        assert_eq!(default.folder, None);
        assert!(!default.recurse);
    }

    #[test]
    fn the_settings_go_where_the_operating_system_keeps_configuration() {
        let path = settings_path();
        assert_eq!(path.file_name().unwrap(), FILE);

        let exe = std::env::current_exe().expect("an executable");
        assert_ne!(
            path.parent(),
            exe.parent(),
            "settings are being written beside the executable"
        );

        let dirs = directories::ProjectDirs::from("", "", "imgdedupe")
            .expect("the platform has a configuration directory");
        assert_eq!(path.parent(), Some(dirs.config_dir()));
    }
}
