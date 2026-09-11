use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};
use imgdedupe_core::decode::{decode_at_most, turn_upright};
use imgdedupe_core::format::{self, Format, SNIFF_LEN};
use imgdedupe_core::preview;

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

/// What the workers read from.
struct Lanes {
    queues: Mutex<Queues>,
    ready: Condvar,
}

/// One picture to read: which file, and which result it belongs to.
struct Wanted {
    key: Key,
    path: PathBuf,
    result: u64,
}

#[derive(Default)]
struct Queues {
    /// The tiles the last frame drew, the first of them at the back.
    wanted: Vec<Wanted>,
    rest: VecDeque<Wanted>,
    /// Nothing from `rest` is started while a tile on screen is still missing.
    hold_rest: bool,
    /// Keys a worker has taken. A picture can be in both lanes, and this is what
    /// stops it being decoded twice.
    started: HashSet<Key>,
    /// Which result the queues are holding. A new one makes every key mean a
    /// different file, so anything from an older one is thrown away.
    result: u64,
    stop: bool,
}

impl Queues {
    /// What a worker that only ever reads the screen takes.
    fn take_wanted(&mut self) -> Option<Wanted> {
        while let Some(next) = self.wanted.pop() {
            if self.started.insert(next.key) {
                return Some(next);
            }
        }
        None
    }

    /// What a worker that only ever reads the background takes. It takes nothing
    /// at all while a tile on screen is still missing.
    fn take_rest(&mut self) -> Option<Wanted> {
        if self.hold_rest {
            return None;
        }
        while let Some(next) = self.rest.pop_front() {
            if self.started.insert(next.key) {
                return Some(next);
            }
        }
        None
    }
}

/// How much reading has been asked for and how much has arrived, so a review that
/// sits on placeholders says where the time is going.
#[derive(Debug, Default)]
struct Tally {
    asked: u64,
    arrived: u64,
    failed: u64,
    decoding: f64,
    uploading: f64,
    started: Option<std::time::Instant>,
    said: u64,
}

impl Tally {
    /// Every so many pictures, and once at the end.
    const EVERY: u64 = 200;

    fn ask(&mut self) {
        if self.asked == 0 {
            self.started = Some(std::time::Instant::now());
        }
        self.asked += 1;
    }

    fn arrive(&mut self, decoding: f64, uploading: f64, ok: bool) {
        self.arrived += 1;
        self.decoding += decoding;
        self.uploading += uploading;
        if !ok {
            self.failed += 1;
        }
    }

    fn say(&mut self, force: bool) {
        // Nothing new since the last time is nothing to say, however forced.
        // Once every picture had arrived, this spoke on every frame for as long
        // as the window was open.
        if self.arrived == 0
            || self.arrived == self.said
            || (!force && self.arrived < self.said + Self::EVERY)
        {
            return;
        }
        self.said = self.arrived;
        #[cfg(feature = "logging")]
        let waited = self.started.map_or(0.0, |at| at.elapsed().as_secs_f64());
        imgdedupe_core::log_line!(
            "thumbnails: {} of {} in {waited:.1}s, {} failed, {:.0}ms decoding and \
             {:.0}ms uploading per picture",
            self.arrived,
            self.asked,
            self.failed,
            self.decoding * 1000.0 / self.arrived as f64,
            self.uploading * 1000.0 / self.arrived as f64
        );
    }
}

/// Threads kept for what is on screen. A review shows a couple of dozen tiles at
/// once, and these decode them all at the same time.
const SCREEN_WORKERS: usize = 24;

/// Threads reading ahead. Four, because every one of them is a picture already
/// being decoded that a scroll has to wait behind.
const BACKGROUND_WORKERS: usize = 4;

/// A file at one size. The grid and the pane beside it want different sizes of
/// the same picture, and those are different pictures as far as the texture is
/// concerned.
type Key = (i64, u32);

struct Decoded {
    key: Key,
    image: Option<ColorImage>,
    /// The result this was asked for. One from an older result is dropped.
    result: u64,
    /// Seconds spent reading and decoding it.
    took: f64,
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

fn load(path: &Path, edge: u32) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let format = format::detect(head)?;
    let decoded = decode_at_most(format, &bytes, edge).ok()?;
    // A camera held on its side writes the picture the way the sensor read it
    // and a number saying which way up it goes. Everything that draws it has to
    // do that turn, and this is the one place either the tiles or the preview
    // gets a picture from.
    let upright = turn_upright(decoded.small, the_way_up(&bytes, format));
    let size = [upright.width() as usize, upright.height() as usize];
    Some(ColorImage::from_rgb(size, upright.as_raw()))
}

/// Which way up the file says its picture goes.
///
/// A raw file is shown through the preview inside it, and the preview is written
/// the way the sensor read it like everything else in there, so the number in
/// the raw's own directory is the one that applies to it.
fn the_way_up(bytes: &[u8], format: Format) -> u16 {
    match format {
        Format::Cr3 | Format::Heic => 1,
        _ => preview::the_way_up(bytes),
    }
}

#[cfg(test)]
#[path = "tests/thumbs.rs"]
mod tests;
