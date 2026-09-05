//! The metadata of the picture the preview is showing, read off the file.
//!
//! The reading of it is all this does. What the bytes mean is
//! `imgdedupe_core::metadata`, which this calls once the file is in hand.
//!
//! The reading happens on a thread of its own. A raw file is tens of megabytes
//! and lives wherever the folder lives, which is often another machine, and the
//! window has to go on drawing while it arrives. What is on screen is whatever
//! was read last until the next one lands.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use imgdedupe_core::format::{self, SNIFF_LEN};
use imgdedupe_core::metadata::{read as parse, Group};

#[derive(Default)]
pub struct Metadata {
    /// The file the groups below are about.
    about: Option<i64>,
    groups: Vec<Group>,
    /// The file being read, and where the answer will arrive.
    reading: Option<(i64, Receiver<(i64, Vec<Group>)>)>,
}

impl Metadata {
    /// What the file says, asking for it if this is the first time it has been
    /// wanted. Empty while it is being read, which is what the caller says so.
    pub fn get(&mut self, file_id: i64, path: PathBuf, ctx: &egui::Context) -> &[Group] {
        if self.about == Some(file_id) {
            return &self.groups;
        }
        if let Some((wanted, waiting)) = &self.reading {
            if *wanted == file_id {
                if let Ok((file_id, groups)) = waiting.try_recv() {
                    self.about = Some(file_id);
                    self.groups = groups;
                    self.reading = None;
                    return &self.groups;
                }
                return &[];
            }
        }

        // A different picture. Whatever was being read is no longer wanted, and
        // dropping the other end of the channel is how the thread finds out.
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let groups = read(&path);
            if send.send((file_id, groups)).is_ok() {
                // The window is not drawing while nothing happens, and this
                // happened.
                ctx.request_repaint();
            }
        });
        self.reading = Some((file_id, receive));
        self.about = None;
        self.groups.clear();
        &[]
    }

    /// Whether anything is being waited for, which is what the pane says while
    /// it waits.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }
}

fn read(path: &PathBuf) -> Vec<Group> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let Some(format) = format::detect(head) else {
        return Vec::new();
    };
    parse(&bytes, format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(path: &std::path::Path) {
        let image = image::RgbImage::from_fn(32, 24, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 8) as u8, 90])
        });
        image::DynamicImage::ImageRgb8(image)
            .save_with_format(path, image::ImageFormat::Png)
            .expect("writing a fixture");
    }

    /// Nothing is read on the thread that draws: asking is asking, and the answer
    /// turns up later. The window has to keep drawing while a file arrives off
    /// another machine.
    #[test]
    fn asking_gives_nothing_back_at_once_and_something_back_later() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.png");
        picture(&path);
        // A PNG with something in it to say. The text goes in as the format
        // keeps it: a name, a zero, and the words.
        let mut bytes = std::fs::read(&path).expect("read");
        let mut chunk: Vec<u8> = Vec::new();
        chunk.extend_from_slice(b"tEXtComment\0a deer in a garden");
        let length = (chunk.len() - 4) as u32;
        let sum = crc(&chunk);
        let mut piece = length.to_be_bytes().to_vec();
        piece.append(&mut chunk);
        piece.extend_from_slice(&sum.to_be_bytes());
        let end = bytes.len() - 12;
        bytes.splice(end..end, piece);
        std::fs::write(&path, &bytes).expect("write");

        let ctx = egui::Context::default();
        let mut held = Metadata::default();
        assert!(
            held.get(1, path.clone(), &ctx).is_empty(),
            "the file was read on this thread"
        );
        assert!(held.reading());

        let waited = std::time::Instant::now();
        loop {
            let groups = held.get(1, path.clone(), &ctx);
            if !groups.is_empty() {
                let text = groups
                    .iter()
                    .flat_map(|group| group.entries.iter())
                    .find(|(name, _)| name == "Comment")
                    .map(|(_, value)| value.clone());
                assert_eq!(text.as_deref(), Some("a deer in a garden"));
                break;
            }
            assert!(waited.elapsed().as_secs() < 10, "nothing was ever read");
            std::thread::yield_now();
        }
    }

    /// The check every PNG chunk carries. Written here because the fixture is a
    /// chunk this test adds by hand.
    fn crc(bytes: &[u8]) -> u32 {
        let mut value = 0xFFFF_FFFFu32;
        for byte in bytes {
            value ^= *byte as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 { 0xEDB8_8320 ^ (value >> 1) } else { value >> 1 };
            }
        }
        value ^ 0xFFFF_FFFF
    }
}
