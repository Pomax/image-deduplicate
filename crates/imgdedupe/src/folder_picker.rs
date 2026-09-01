use std::path::{Path, PathBuf};

/// Ask for a folder, starting at `start` if it is given.
///
/// Not `rfd::FileDialog::pick_folder` on Windows. That reads the chosen item with
/// `SIGDN_FILESYSPATH`, which resolves the path to its underlying storage: a
/// mapped drive comes back as the UNC share behind it, and a link comes back as
/// its target. The path someone clicked is the one they meant, so the dialog is
/// opened here and read with `SIGDN_DESKTOPABSOLUTEPARSING`, which does not
/// resolve.
pub fn pick(start: Option<&Path>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_picker::pick(start)
    }
    #[cfg(not(windows))]
    {
        let dialog = match start {
            Some(folder) => rfd::FileDialog::new().set_directory(folder),
            None => rfd::FileDialog::new(),
        };
        dialog.pick_folder()
    }
}

#[cfg(windows)]
mod windows_picker {
    use std::path::{Path, PathBuf};

    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        FILEOPENDIALOGOPTIONS, FOS_NODEREFERENCELINKS, FOS_PICKFOLDERS,
        SIGDN_DESKTOPABSOLUTEPARSING,
    };

    pub fn pick(start: Option<&Path>) -> Option<PathBuf> {
        unsafe {
            // The dialog needs an apartment-threaded COM context. It may already
            // be initialised on this thread, which is not an error here.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;

            let options: FILEOPENDIALOGOPTIONS =
                dialog.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_NODEREFERENCELINKS;
            dialog.SetOptions(options).ok()?;

            // `SetDefaultFolder`, not `SetFolder`. `SetFolder` navigates there,
            // and navigating to a junction or a directory symlink goes through it
            // to what it points at, so the dialog opens on the target and the
            // link is gone. `FOS_NODEREFERENCELINKS` does not cover that: it is
            // about `.lnk` shortcuts, not reparse points.
            if let Some(folder) = start {
                if let Some(item) = shell_item(folder) {
                    let _ = dialog.SetDefaultFolder(&item);
                }
            }

            dialog.Show(None).ok()?;
            let chosen = dialog.GetResult().ok()?;

            let wide = chosen.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok()?;
            let path = wide.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(wide.0 as *const _));
            path
        }
    }

    fn shell_item(folder: &Path) -> Option<IShellItem> {
        let mut wide: Vec<u16> = folder.as_os_str().encode_wide().collect();
        wide.push(0);
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok() }
    }

    use std::os::windows::ffi::OsStrExt;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picker_exists_for_this_platform() {
        // The function is here and takes a starting folder. Opening a dialog
        // needs a person, so that part is not exercised.
        let _: fn(Option<&Path>) -> Option<PathBuf> = pick;
    }
}
