use super::*;

#[test]
fn a_picker_exists_for_this_platform() {
    // The function is here and takes a starting folder. Opening a dialog needs a
    // person, so that part is not exercised.
    let _: fn(Option<&Path>) -> Option<PathBuf> = pick;
}
