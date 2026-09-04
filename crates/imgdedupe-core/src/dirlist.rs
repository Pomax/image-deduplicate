use std::io;
use std::path::Path;

/// One entry of a directory, with the facts the diff needs about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub size_bytes: i64,
    pub mtime_ns: i64,
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

/// A modification time as nanoseconds since the epoch, which is how the index
/// stores it.
pub fn mtime_nanos(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Listed;
    use std::io;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    /// `struct attrlist` from `<sys/attr.h>`.
    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }

    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
    /// How many things are in a directory, straight out of its own record.
    const ATTR_DIR_ENTRYCOUNT: u32 = 0x0000_0002;
    const ATTR_CMN_NAME: u32 = 0x0000_0001;
    const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
    const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
    const ATTR_CMN_FILEID: u32 = 0x0200_0000;
    const ATTR_FILE_DATALENGTH: u32 = 0x0000_0200;

    /// Do not follow symbolic links, and write zeros for anything the file system
    /// cannot answer rather than leaving it out. Without the second one an entry's
    /// fields move depending on what was available, and there is no way to read
    /// them back.
    const FSOPT_NOFOLLOW: u64 = 0x0000_0001;
    const FSOPT_PACK_INVAL_ATTRS: u64 = 0x0000_0008;

    /// `fsobj_type_t` values: a regular file and a directory.
    const VREG: u32 = 1;
    const VDIR: u32 = 2;

    extern "C" {
        fn getattrlist(
            path: *const std::os::raw::c_char,
            alist: *mut std::os::raw::c_void,
            attr_buf: *mut std::os::raw::c_void,
            attr_buf_size: usize,
            options: u64,
        ) -> std::os::raw::c_int;

        fn getattrlistbulk(
            dirfd: std::os::raw::c_int,
            alist: *mut std::os::raw::c_void,
            attr_buf: *mut std::os::raw::c_void,
            attr_buf_size: usize,
            options: u64,
        ) -> std::os::raw::c_int;
    }

    /// Big enough that a folder of a few thousand comes back in a handful of
    /// calls, small enough to be nothing on the heap.
    const BUFFER: usize = 256 * 1024;

    /// How many things the directory says it holds, in one call, before anything
    /// has been listed. That is the denominator a bar needs from the first moment.
    /// `None` when the file system does not keep one.
    pub fn entry_count(dir: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut request = AttrList {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS,
            volattr: 0,
            dirattr: ATTR_DIR_ENTRYCOUNT,
            fileattr: 0,
            forkattr: 0,
        };
        let mut buffer = [0u8; 64];
        let ok = unsafe {
            getattrlist(
                path.as_ptr(),
                (&mut request as *mut AttrList).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS,
            )
        };
        if ok != 0 {
            return None;
        }
        // Five groups of returned attributes in the order of `struct attrlist`,
        // so the directory group is the third.
        let returned = unsafe { buffer.as_ptr().add(4 + 8).cast::<u32>().read_unaligned() };
        if returned & ATTR_DIR_ENTRYCOUNT == 0 {
            return None;
        }
        Some(u64::from(unsafe {
            buffer.as_ptr().add(4 + 20).cast::<u32>().read_unaligned()
        }))
    }

    pub fn list(
        dir: &Path,
        stop: &dyn Fn() -> bool,
        found: &dyn Fn(u64),
    ) -> io::Result<Vec<Listed>> {
        let handle = std::fs::File::open(dir)?;
        let fd = handle.as_raw_fd();

        let mut request = AttrList {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_MODTIME
                | ATTR_CMN_FILEID,
            volattr: 0,
            dirattr: 0,
            fileattr: ATTR_FILE_DATALENGTH,
            forkattr: 0,
        };

        let mut buffer = vec![0u8; BUFFER];
        let mut out = Vec::new();
        loop {
            // Between the round trips, which is the only place a listing can be
            // stopped: the call itself is one and will not be interrupted.
            if stop() {
                return Ok(out);
            }
            let count = unsafe {
                getattrlistbulk(
                    fd,
                    (&mut request as *mut AttrList).cast(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS,
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                found(out.len() as u64);
                return Ok(out);
            }
            let mut at = buffer.as_ptr();
            for _ in 0..count {
                // Safety: the kernel wrote `count` entries into the buffer, each
                // starting with its own length, and each field is read at the
                // offset the attributes were asked for in.
                let (entry, length) = unsafe { read_entry(at) };
                if let Some(entry) = entry {
                    out.push(entry);
                }
                at = unsafe { at.add(length) };
            }
            // Each call is one round trip, and on a folder that answers slowly
            // that is where the whole of the wait is.
            found(out.len() as u64);
        }
    }

    /// Read one entry and say how long it was, so the next one can be found.
    ///
    /// The fields come back in the order of the attribute bits, not the order
    /// they were asked in, and they are packed with no padding between them, so
    /// every read here is unaligned.
    unsafe fn read_entry(at: *const u8) -> (Option<Listed>, usize) {
        let length = at.cast::<u32>().read_unaligned() as usize;
        // The returned-attributes set, asked for first so the rest of the entry
        // has a fixed shape.
        let mut field = at.add(4 + std::mem::size_of::<u32>() * 5);

        // ATTR_CMN_NAME, an `attrreference_t`: an offset from its own address and
        // a length that counts the terminating zero.
        let name_at = field;
        let offset = name_at.cast::<i32>().read_unaligned() as isize;
        let name_len = name_at.add(4).cast::<u32>().read_unaligned() as usize;
        field = field.add(8);
        let name = if name_len == 0 {
            String::new()
        } else {
            let bytes = std::slice::from_raw_parts(name_at.offset(offset), name_len - 1);
            match std::str::from_utf8(bytes) {
                Ok(name) => name.to_string(),
                // A name this build cannot spell is one it cannot store in the
                // index either, since paths are kept as text.
                Err(_) => return (None, length),
            }
        };

        // ATTR_CMN_OBJTYPE, an `fsobj_type_t`.
        let objtype = field.cast::<u32>().read_unaligned();
        field = field.add(4);

        // ATTR_CMN_MODTIME, a `struct timespec` of two 64 bit numbers.
        let seconds = field.cast::<i64>().read_unaligned();
        let nanos = field.add(8).cast::<i64>().read_unaligned();
        field = field.add(16);

        // ATTR_CMN_FILEID, a `u64`. Comes after the times because the fields
        // arrive in the order of the attribute bits, not the order they were
        // asked in.
        let file_id = field.cast::<u64>().read_unaligned();
        field = field.add(8);

        // ATTR_FILE_DATALENGTH, an `off_t`. Zero for anything that is not a file.
        let size = field.cast::<i64>().read_unaligned();

        let entry = Listed {
            name,
            is_dir: objtype == VDIR,
            is_file: objtype == VREG,
            size_bytes: size,
            file_id,
            mtime_ns: seconds
                .saturating_mul(1_000_000_000)
                .saturating_add(nanos.clamp(0, 999_999_999)),
        };
        (Some(entry), length)
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Listed;
    use std::io;
    use std::path::Path;

    /// Nothing portable answers this without listing the directory, which is the
    /// thing the number would be measuring.
    pub fn entry_count(_dir: &Path) -> Option<u64> {
        None
    }

    pub fn list(
        dir: &Path,
        stop: &dyn Fn() -> bool,
        found: &dyn Fn(u64),
    ) -> io::Result<Vec<Listed>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            if stop() {
                return Ok(out);
            }
            if out.len() % 64 == 0 {
                found(out.len() as u64);
            }
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            out.push(Listed {
                name,
                is_dir: metadata.is_dir(),
                is_file: metadata.is_file(),
                size_bytes: metadata.len() as i64,
                mtime_ns: super::mtime_nanos(&metadata),
                file_id: {
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::MetadataExt::ino(&metadata)
                    }
                    #[cfg(not(unix))]
                    {
                        0
                    }
                },
            });
        }
        found(out.len() as u64);
        Ok(out)
    }
}
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
        let want = Radvisory { ra_offset: 0, ra_count: length.min(i32::MAX as i64) as _ };
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
    let want = Radvisory { ra_offset: 0, ra_count: length.min(i32::MAX as i64) as _ };
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

