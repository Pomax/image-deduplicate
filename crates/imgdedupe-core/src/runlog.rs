use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// Where the log goes: beside the executable, named after it.
fn path_for(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(format!("{name}.log"))))
        .unwrap_or_else(|| PathBuf::from(format!("{name}.log")))
}

/// Start logging, and record a panic in the log before the process dies.
///
/// A crash with no trace of what it was doing is the thing this exists to stop,
/// so the hook runs for panics on any thread, not only the main one.
///
/// Nothing is written unless this is called. Neither program calls it without
/// being asked to on the command line: a tool that leaves a file beside itself
/// every time it runs was not what anyone wanted.
pub fn start(name: &str) {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path_for(name))
        .ok();
    let _ = LOG.set(Mutex::new(file));

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        line(&format!(
            "PANIC on thread {}: {info}",
            thread.name().unwrap_or("unnamed")
        ));
        previous(info);
    }));

    line(&format!("--- {name} started ---"));
}

/// Whether `start` has been called. A program that spawns another passes the
/// flag on, so one run leaves one set of logs rather than half of them.
pub fn is_on() -> bool {
    LOG.get().is_some()
}

/// Append one line. Never fails and never panics: a logger that can bring the
/// program down is worse than no logger.
pub fn line(text: &str) {
    let Some(lock) = LOG.get() else {
        return;
    };
    let Ok(mut guard) = lock.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{seconds}] {text}");
    let _ = file.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_sits_beside_the_executable() {
        let path = path_for("thing");
        assert_eq!(path.file_name().unwrap(), "thing.log");
        let exe = std::env::current_exe().expect("an executable");
        assert_eq!(path.parent(), exe.parent());
    }

    #[test]
    fn writing_before_starting_does_nothing_rather_than_failing() {
        // `line` is called from the panic hook and from worker threads, so it has
        // to be safe to call at any point, including before or after setup.
        line("this goes nowhere and must not panic");
    }
}
