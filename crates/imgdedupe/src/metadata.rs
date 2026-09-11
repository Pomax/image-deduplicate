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
#[path = "tests/metadata.rs"]
mod tests;
