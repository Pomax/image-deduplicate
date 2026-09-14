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

pub fn list(dir: &Path, stop: &dyn Fn() -> bool, found: &dyn Fn(u64)) -> io::Result<Vec<Listed>> {
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

    // ATTR_CMN_MODTIME, a `struct timespec` of two 64 bit numbers. The
    // nanoseconds are stepped over: the index keeps whole seconds.
    let seconds = field.cast::<i64>().read_unaligned();
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
        mtime_seconds: seconds,
    };
    (Some(entry), length)
}
