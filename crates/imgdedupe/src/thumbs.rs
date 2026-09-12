use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};
use imgdedupe_core::decode::{decode_at_most, turn_upright};
use imgdedupe_core::format::{self, Format, SNIFF_LEN};
use imgdedupe_core::preview;

mod load;
mod queues;

use self::load::*;
use self::queues::*;

/// Long edge of a grid thumbnail.
pub const THUMB_EDGE: u32 = 256;

/// Long edge of the picture shown beside the list.
///
/// Chosen so a photograph decodes at half scale rather than in full. A 3000x4000
/// file asked for at 2048 has to be decoded whole, because half of it is 2000 and
/// that is short of what was asked for. Asked for at 1600 it comes off the half
/// scale path, which is a quarter of the pixels, and 1600 is still more than the
/// pane can show.
pub const LARGE_EDGE: u32 = 1600;

/// The pictures the review list draws.
///
/// What is on screen is the only thing the workers touch until it is on screen.
/// Every frame hands them the tiles it drew; a tile that has scrolled out of
/// view and has not been picked up yet is taken off them again, and the rest of
/// the result is only read once nothing being drawn is still missing.
///
/// Everything read stays in memory, so going back to a picture never waits.
///
/// `egui` redraws every frame, so decoding on the frame that needs an image would
/// stall the window. Requests go to the workers and the grid shows a placeholder
/// until the answer arrives.
pub struct Thumbnails {
    textures: HashMap<Key, TextureHandle>,
    /// Decoded and waiting. A picture becomes a texture on the frame that draws
    /// it, so a background pass cannot spend the window's frames uploading
    /// thousands of pictures nobody is looking at.
    ready: HashMap<Key, ColorImage>,
    /// Kept from `collect` so a tile can be uploaded while it is being drawn.
    painter: Option<egui::Context>,
    pending: HashSet<Key>,
    /// Keys that came back with nothing, so a file that cannot be read is not
    /// asked for again on every frame it is on screen.
    failed: HashSet<Key>,
    /// What the frame being drawn has asked for, in the order it drew it.
    drawn: Vec<(Key, PathBuf)>,
    drawing: HashSet<Key>,
    /// What the workers were last given, so an unchanged view costs nothing.
    in_front: HashSet<Key>,
    holding: bool,
    /// Whether a frame has drawn a tile yet. Until one has, the rest is held, or
    /// the workers would all be inside a background picture on the frame the
    /// review opens.
    drew: bool,
    /// Which search result these pictures belong to. A file id only names a file
    /// for as long as the index it came from is the current one.
    result: u64,
    lanes: Arc<Lanes>,
    results: Receiver<Decoded>,
    /// What the reading is costing, written to the run log as it goes.
    tally: Tally,
}

impl Thumbnails {
    pub fn new() -> Self {
        let (result_tx, results) = mpsc::channel::<Decoded>();
        let lanes = Arc::new(Lanes {
            queues: Mutex::new(Queues {
                hold_rest: true,
                ..Queues::default()
            }),
            ready: Condvar::new(),
        });

        // Threads of their own for the screen, so a tile the eye is on never
        // waits for a background picture a worker happens to be inside. They
        // sit idle whenever the screen is complete.
        //
        // The background gets what is left, minus two cores for the window and
        // the rest of the machine. How fast a screenful appears is how many of
        // it can be decoded at once, so the screen pool is a screenful wide.
        let cores = std::thread::available_parallelism().map_or(4, |count| count.get());
        // A background picture cannot be given back once a worker is inside it,
        // so the size of that pool is how much work the screen can find already
        // running when someone scrolls. It is deliberately small.
        let on_screen = cores.saturating_sub(2).clamp(1, SCREEN_WORKERS);
        let background = BACKGROUND_WORKERS.min(cores).max(1);
        for worker in 0..on_screen + background {
            let screen = worker < on_screen;
            let lanes = Arc::clone(&lanes);
            let result_tx = result_tx.clone();
            std::thread::spawn(move || loop {
                let Wanted { key, path, result } = {
                    let mut queues = lanes.queues.lock().expect("the thumbnail queues");
                    loop {
                        if queues.stop {
                            return;
                        }
                        let next = if screen {
                            queues.take_wanted()
                        } else {
                            queues.take_rest()
                        };
                        if let Some(next) = next {
                            break next;
                        }
                        queues = lanes.ready.wait(queues).expect("the thumbnail queues");
                    }
                };
                let started = std::time::Instant::now();
                let image = load(&path, key.1);
                let took = started.elapsed().as_secs_f64();
                if result_tx
                    .send(Decoded {
                        key,
                        image,
                        result,
                        took,
                    })
                    .is_err()
                {
                    return;
                }
            });
        }

        Thumbnails {
            textures: HashMap::new(),
            ready: HashMap::new(),
            painter: None,
            pending: HashSet::new(),
            failed: HashSet::new(),
            drawn: Vec::new(),
            drawing: HashSet::new(),
            in_front: HashSet::new(),
            holding: true,
            drew: false,
            result: 0,
            lanes,
            results,
            tally: Tally::default(),
        }
    }

    /// Take everything the workers have finished, then hand them the tiles the
    /// frame just drew. Called once per frame.
    pub fn collect(&mut self, ctx: &egui::Context) {
        if self.painter.is_none() {
            self.painter = Some(ctx.clone());
        }
        while let Ok(decoded) = self.results.try_recv() {
            // Asked for before the last search, so its key names whatever file
            // holds that row now, which is not the file it was read from.
            if decoded.result != self.result {
                continue;
            }
            self.pending.remove(&decoded.key);
            let Some(image) = decoded.image else {
                self.failed.insert(decoded.key);
                self.tally.arrive(decoded.took, 0.0, false);
                continue;
            };
            self.ready.insert(decoded.key, image);
            self.tally.arrive(decoded.took, 0.0, true);
        }
        self.hand_over();
        self.tally.say(self.pending.is_empty());

        // Nothing else will wake the window when a picture finishes reading, and
        // then it appears on whatever frame some other input happens to cause.
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Give the workers what the last frame drew and take back what it did not.
    ///
    /// A tile that scrolled out of view before a worker reached it goes to the
    /// head of the rest rather than being read now, and nothing in the rest is
    /// started at all while a tile on screen is still missing.
    fn hand_over(&mut self) {
        self.drew |= !self.drawing.is_empty();
        let missing = !self.drew
            || self
                .drawing
                .iter()
                .any(|key| !self.textures.contains_key(key));
        if self.drawing == self.in_front && missing == self.holding {
            self.drawn.clear();
            self.drawing.clear();
            return;
        }

        let lanes = Arc::clone(&self.lanes);
        let mut queues = lanes.queues.lock().expect("the thumbnail queues");
        for entry in std::mem::take(&mut queues.wanted) {
            if !self.drawing.contains(&entry.key) {
                queues.rest.push_front(entry);
            }
        }
        let result = self.result;
        queues
            .wanted
            .extend(
                self.drawn
                    .drain(..)
                    .rev()
                    .map(|(key, path)| Wanted { key, path, result }),
            );
        queues.hold_rest = missing;
        drop(queues);

        self.holding = missing;
        self.in_front = std::mem::take(&mut self.drawing);
        lanes.ready.notify_all();
    }

    /// The texture for a file at one size, asking for it if this is the first
    /// sight of it.
    pub fn get(
        &mut self,
        file_id: i64,
        edge: u32,
        root: &Path,
        rel_path: &str,
    ) -> Option<TextureHandle> {
        let key = (file_id, edge);
        if let Some(handle) = self.textures.get(&key).cloned() {
            return Some(handle);
        }
        if let Some(handle) = self.upload(key) {
            return Some(handle);
        }
        self.request(key, root, rel_path);
        None
    }

    /// Turn a decoded picture into a texture, on the frame that draws it.
    fn upload(&mut self, key: Key) -> Option<TextureHandle> {
        let painter = self.painter.clone()?;
        let image = self.ready.remove(&key)?;
        let (file_id, edge) = key;
        let at = std::time::Instant::now();
        let handle = painter.load_texture(
            format!("preview{edge}-{file_id}"),
            image,
            TextureOptions::default(),
        );
        self.tally.uploading += at.elapsed().as_secs_f64();
        self.textures.insert(key, handle.clone());
        Some(handle)
    }

    /// Ask for every picture in the list, behind whatever is being drawn.
    /// Called once, when the sets are found.
    pub fn prime<'a>(
        &mut self,
        root: &Path,
        members: impl Iterator<Item = (i64, &'a str)>,
        edge: u32,
    ) {
        let lanes = Arc::clone(&self.lanes);
        let mut queues = lanes.queues.lock().expect("the thumbnail queues");
        for (file_id, rel_path) in members {
            let key = (file_id, edge);
            if self.textures.contains_key(&key) || !self.pending.insert(key) {
                continue;
            }
            self.tally.ask();
            queues.rest.push_back(Wanted {
                key,
                path: root.join(rel_path),
                result: self.result,
            });
        }
        drop(queues);
        lanes.ready.notify_all();
    }

    /// Throw away everything read for the last search result.
    ///
    /// A picture is named by its row in the index, and a row id only means one
    /// file for as long as that index stands. A new result can hand the same id
    /// to a different picture, so nothing read under the old one may be shown
    /// under the new one.
    pub fn forget(&mut self) {
        self.textures.clear();
        self.ready.clear();
        self.pending.clear();
        self.failed.clear();
        self.drawn.clear();
        self.drawing.clear();
        self.in_front.clear();
        self.holding = true;
        self.drew = false;
        self.result += 1;

        let lanes = Arc::clone(&self.lanes);
        let mut queues = lanes.queues.lock().expect("the thumbnail queues");
        queues.wanted.clear();
        queues.rest.clear();
        queues.started.clear();
        queues.hold_rest = true;
        queues.result = self.result;
        drop(queues);
        lanes.ready.notify_all();
    }

    /// Note that this frame is drawing a picture it does not have. The workers
    /// are given the whole frame's worth at once, in `hand_over`.
    fn request(&mut self, key: Key, root: &Path, rel_path: &str) {
        if self.failed.contains(&key) || !self.drawing.insert(key) {
            return;
        }
        self.drawn.push((key, root.join(rel_path)));
        if self.pending.insert(key) {
            self.tally.ask();
        }
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        if let Ok(mut queues) = self.lanes.queues.lock() {
            queues.stop = true;
        }
        self.lanes.ready.notify_all();
    }
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/thumbs.rs"]
mod tests;
