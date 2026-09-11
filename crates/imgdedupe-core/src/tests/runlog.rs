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
