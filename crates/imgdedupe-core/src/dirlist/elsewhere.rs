use super::Listed;
use std::io;
use std::path::Path;

/// Nothing portable answers this without listing the directory, which is the
/// thing the number would be measuring.
pub fn entry_count(_dir: &Path) -> Option<u64> {
    None
}

pub fn list(dir: &Path, stop: &dyn Fn() -> bool, found: &dyn Fn(u64)) -> io::Result<Vec<Listed>> {
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
            mtime_seconds: super::mtime_seconds(&metadata),
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
