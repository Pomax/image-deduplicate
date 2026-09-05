//! Keeps Mesa's driver-probe chatter off the console.

use std::path::Path;

/// Set before the window is created, and only where the variable is not already
/// set, so anything chosen on the command line still wins.
pub fn quieten() {
    if std::env::var_os("EGL_LOG_LEVEL").is_none() {
        std::env::set_var("EGL_LOG_LEVEL", "fatal");
    }
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none() && !has_render_node() {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }
}

/// A `renderD*` node is what a client needs to reach the GPU. Without one the
/// only path is software, and asking for it directly skips the probe that fails.
fn has_render_node() -> bool {
    let Ok(entries) = std::fs::read_dir(Path::new("/dev/dri")) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("renderD"))
    })
}
