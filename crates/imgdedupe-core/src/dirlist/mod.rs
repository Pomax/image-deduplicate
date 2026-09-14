use std::io;
use std::path::Path;

/// One entry of a directory, with the facts the diff needs about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub size_bytes: i64,
    pub mtime_seconds: i64,
    /// The file system's own number for this file, which on most file systems
    /// rises with where the file sits on the storage. Reading in this order is
    /// closer to reading the disk in a line than jumping about it, which is what
    /// reading in directory order amounts to. Zero when nothing answered.
    pub file_id: u64,
}

/// List a directory, with each entry's size and modification time.
///
/// The portable way is `readdir` for the names and then one `lstat` per name for
/// the rest, because that is all POSIX offers. Every one of those calls is a
/// round trip when the directory is on another machine, so a folder of nine
/// thousand costs nine thousand round trips, and the size and time were already
/// in the directory response that `readdir` threw away.
///
/// macOS has one call that returns both, `getattrlistbulk`, which is what this
/// uses and what makes the same folder appear at once in Finder. Everywhere else
/// falls back to the per-file version.
/// `found` is called with the number of entries so far, as they arrive. Listing a
/// folder on another machine takes as long as it takes and this is the only thing
/// there is to show for it: the call returns nothing until it has everything, so
/// anything reporting outside it reports once, at the end.
/// `stop` is looked at between batches, which is between the round trips, so a
/// listing of a folder that answers slowly stops when it is asked rather than
/// when it happens to be finished. What it had listed by then comes back.
pub fn list(dir: &Path, stop: &dyn Fn() -> bool, found: &dyn Fn(u64)) -> io::Result<Vec<Listed>> {
    imp::list(dir, stop, found)
}

/// How many entries the directory says it holds, without listing it, so a bar
/// measuring the listing has a denominator before the listing starts.
pub fn entry_count(dir: &Path) -> Option<u64> {
    imp::entry_count(dir)
}

/// A modification time as whole seconds since the epoch, which is how the index
/// stores it. Truncated, not rounded.
pub fn mtime_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;

#[cfg(not(target_os = "macos"))]
#[path = "elsewhere.rs"]
mod imp;

/// Read a whole file, having first told the system that the whole of it is
/// wanted.
///
/// A plain read asks for bytes as the reader gets to them, and a client talking
/// to another machine answers in whatever size it feels like, one wait after
/// another. `F_RDADVISE` says up front how much is coming, so the fetching starts
/// at once and in the background, and the read that follows takes it out of the
/// cache instead of off the wire. The length is already known from the listing,
/// so nothing has to be asked to find it out.
#[cfg(target_os = "macos")]
pub fn read_whole(path: &Path, length: i64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::io::AsRawFd;

    let mut file = std::fs::File::open(path)?;
    if length > 0 {
        let want = Radvisory {
            ra_offset: 0,
            ra_count: length.min(i32::MAX as i64) as _,
        };
        // Advice, not a request: a file system that does not take it says so and
        // the read below is what it always was.
        unsafe { fcntl(file.as_raw_fd(), F_RDADVISE, &want) };
    }
    let mut bytes = Vec::with_capacity(length.max(0) as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
pub fn read_whole(path: &Path, _length: i64) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Ask the system to start fetching a file, without reading it.
///
/// Opens it, says how much of it is wanted, and closes it again. The fetching
/// carries on in the background and the read that comes later takes it out of the
/// cache. Nothing here waits: a file system that does not take the advice says so
/// and the reader that arrives later does exactly what it always did.
#[cfg(target_os = "macos")]
pub fn ask_for_it_early(path: &Path, length: i64) {
    use std::os::unix::io::AsRawFd;

    if length <= 0 {
        return;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let want = Radvisory {
        ra_offset: 0,
        ra_count: length.min(i32::MAX as i64) as _,
    };
    unsafe { fcntl(file.as_raw_fd(), F_RDADVISE, &want) };
}

#[cfg(not(target_os = "macos"))]
pub fn ask_for_it_early(_path: &Path, _length: i64) {}

/// `struct radvisory` from `<sys/fcntl.h>`.
#[cfg(target_os = "macos")]
#[repr(C)]
struct Radvisory {
    ra_offset: i64,
    ra_count: std::os::raw::c_int,
}

#[cfg(target_os = "macos")]
const F_RDADVISE: std::os::raw::c_int = 44;

#[cfg(target_os = "macos")]
extern "C" {
    fn fcntl(fd: std::os::raw::c_int, cmd: std::os::raw::c_int, ...) -> std::os::raw::c_int;
}
