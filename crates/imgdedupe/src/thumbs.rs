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
            queues: Mutex::new(Queues { hold_rest: true, ..Queues::default() }),
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
                        let next =
                            if screen { queues.take_wanted() } else { queues.take_rest() };
                        if let Some(next) = next {
                            break next;
                        }
                        queues = lanes.ready.wait(queues).expect("the thumbnail queues");
                    }
                };
                let started = std::time::Instant::now();
                let image = load(&path, key.1);
                let took = started.elapsed().as_secs_f64();
                if result_tx.send(Decoded { key, image, result, took }).is_err() {
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
        let missing =
            !self.drew || self.drawing.iter().any(|key| !self.textures.contains_key(key));
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
        queues.wanted.extend(
            self.drawn.drain(..).rev().map(|(key, path)| Wanted { key, path, result }),
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
        let handle =
            painter.load_texture(format!("preview{edge}-{file_id}"), image, TextureOptions::default());
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
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn write(path: &Path, width: u32, height: u32) {
        let image = RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 120])
        });
        DynamicImage::ImageRgb8(image)
            .save_with_format(path, image::ImageFormat::Png)
            .expect("writing a fixture");
    }

    /// A camera held on its side writes a wide picture and a number saying to
    /// turn it. Both the tiles and the preview come through here, so this is
    /// where the turn has to happen, and a picture that arrives on its end is
    /// the proof it did.
    #[test]
    fn a_picture_the_file_says_to_turn_arrives_turned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wide = image::RgbImage::from_fn(400, 200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(wide)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .expect("encoding a fixture");
        let mut bytes = bytes.into_inner();

        // The segment a camera writes to say which way up the picture goes: a
        // quarter turn clockwise.
        let mut segment = vec![0xFF, 0xE1, 0x00, 0x20];
        segment.extend_from_slice(b"Exif\0\0II\x2a\x00");
        segment.extend_from_slice(&8u32.to_le_bytes());
        segment.extend_from_slice(&1u16.to_le_bytes());
        segment.extend_from_slice(&0x0112u16.to_le_bytes());
        segment.extend_from_slice(&3u16.to_le_bytes());
        segment.extend_from_slice(&1u32.to_le_bytes());
        segment.extend_from_slice(&6u16.to_le_bytes());
        segment.extend_from_slice(&[0, 0]);
        segment.extend_from_slice(&0u32.to_le_bytes());
        bytes.splice(2..2, segment);

        let path = dir.path().join("sideways.jpg");
        std::fs::write(&path, &bytes).expect("writing a fixture");

        let shown = load(&path, THUMB_EDGE).expect("decoded");
        assert!(
            shown.size[1] > shown.size[0],
            "a picture the file says to stand on its end came back lying down: {:?}",
            shown.size
        );
    }

    #[test]
    fn loading_reduces_to_the_preview_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.png");
        write(&path, 900, 600);

        let image = load(&path, THUMB_EDGE).expect("decoded");
        assert_eq!(image.size[0], THUMB_EDGE as usize);
        assert_eq!(image.size[1], (THUMB_EDGE * 2 / 3) as usize);
    }

    /// A photograph is 3000x4000 or thereabouts, and the JPEG decoder can hand
    /// back a half, a quarter or an eighth of that without reading the whole
    /// thing. The preview edge has to stay under half the long edge, or every
    /// preview decodes twelve million pixels and the pane sits empty while it
    /// does. This is the check on that.
    #[test]
    fn the_preview_edge_leaves_a_photograph_on_the_half_scale_path() {
        let (width, height) = (3000u32, 4000u32);
        assert!(
            LARGE_EDGE <= height / 2,
            "asking for {LARGE_EDGE} of a {width}x{height} picture decodes it whole"
        );

        // And still larger than a pane can show, or the preview would be soft.
        assert!(LARGE_EDGE >= 1200, "{LARGE_EDGE} is smaller than the pane it fills");
    }

    /// The picture beside the list is decoded from the same file at a larger
    /// edge, so a thumbnail is not what gets stretched across the pane.
    #[test]
    fn the_large_edge_gives_a_bigger_image_than_the_thumbnail_edge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.png");
        write(&path, 3000, 2000);

        let thumb = load(&path, THUMB_EDGE).expect("decoded");
        let large = load(&path, LARGE_EDGE).expect("decoded");
        assert_eq!(thumb.size[0], THUMB_EDGE as usize);
        assert_eq!(large.size[0], LARGE_EDGE as usize);
    }

    /// The list does not wait for a picture to be scrolled to. Everything is
    /// asked for as soon as the sets are known, and nothing that has been read
    /// is thrown away again.
    #[test]
    fn priming_reads_every_picture_and_keeps_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut wanted = Vec::new();
        for index in 0..12 {
            let name = format!("{index}.png");
            write(&dir.path().join(&name), 300, 200);
            wanted.push((index as i64, name));
        }

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();
        thumbs.prime(
            dir.path(),
            wanted.iter().map(|(id, name)| (*id, name.as_str())),
            THUMB_EDGE,
        );

        let first = &wanted[0];
        while !thumbs.pending.is_empty() {
            thumbs.collect(&ctx);
            thumbs.get(first.0, THUMB_EDGE, dir.path(), &first.1);
        }
        assert_eq!(thumbs.textures.len() + thumbs.ready.len(), wanted.len());
        assert_eq!(thumbs.tally.failed, 0);
    }

    /// What is being drawn is read before what is not. Everything is asked for
    /// at once, then the picture at the far end of the list is drawn, and it has
    /// to come back near the front of the answers instead of last.
    #[test]
    fn a_picture_being_drawn_is_read_before_the_ones_that_are_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let count = 400usize;
        let mut wanted = Vec::new();
        for index in 0..count {
            let name = format!("{index}.png");
            write(&dir.path().join(&name), 300, 200);
            wanted.push((index as i64, name));
        }

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();
        thumbs.prime(
            dir.path(),
            wanted.iter().map(|(id, name)| (*id, name.as_str())),
            THUMB_EDGE,
        );

        let last = &wanted[count - 1];
        let (took, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(last));
        let others = thumbs.textures.len() - 1;
        assert!(
            others < 50,
            "the picture on screen arrived in {took:.2}s, behind {others} that are not on screen"
        );
    }

    /// A file id names a file only for as long as the index it came from is the
    /// current one. A new result takes everything read under the old one with
    /// it, including whatever a worker was in the middle of.
    #[test]
    fn a_new_result_keeps_nothing_the_last_one_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("slow.png"), 3000, 2000);
        write(&dir.path().join("quick.png"), 300, 200);
        let slow = (1i64, String::from("slow.png"));
        let quick = (2i64, String::from("quick.png"));

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();

        // Read one picture right through, and start a slow one.
        let (_, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(&quick));
        assert_eq!(thumbs.textures.len(), 1);
        thumbs.collect(&ctx);
        assert!(thumbs.get(slow.0, THUMB_EDGE, dir.path(), &slow.1).is_none());
        thumbs.collect(&ctx);

        thumbs.forget();
        assert!(thumbs.textures.is_empty(), "a picture from the last result was kept");
        assert!(thumbs.ready.is_empty());
        assert!(thumbs.pending.is_empty());

        // Whatever the workers were inside arrives after the result changed, and
        // is thrown away rather than filed under a number that means something
        // else now.
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            thumbs.collect(&ctx);
        }
        assert!(
            thumbs.ready.is_empty() && thumbs.textures.is_empty(),
            "a picture read for the last result was kept for this one"
        );
    }

    /// A decoded picture becomes a texture on the frame that draws it, not on the
    /// frame it arrives. A pass over thousands of pictures must not spend the
    /// window's frames uploading ones nobody is looking at.
    #[test]
    fn a_picture_becomes_a_texture_when_it_is_drawn_and_not_before() {
        let dir = tempfile::tempdir().expect("tempdir");
        let count = 12usize;
        let mut wanted = Vec::new();
        for index in 0..count {
            let name = format!("{index}.png");
            write(&dir.path().join(&name), 300, 200);
            wanted.push((index as i64, name));
        }

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();
        thumbs.prime(
            dir.path(),
            wanted.iter().map(|(id, name)| (*id, name.as_str())),
            THUMB_EDGE,
        );

        let first = &wanted[0];
        while !thumbs.pending.is_empty() {
            thumbs.collect(&ctx);
            thumbs.get(first.0, THUMB_EDGE, dir.path(), &first.1);
        }

        assert_eq!(thumbs.textures.len(), 1, "a picture nobody drew was uploaded");
        assert_eq!(thumbs.ready.len(), count - 1);

        let other = &wanted[5];
        assert!(thumbs.get(other.0, THUMB_EDGE, dir.path(), &other.1).is_some());
        assert_eq!(thumbs.textures.len(), 2);
    }

    /// A picture put in front is left in the queue behind as well. That place
    /// must not turn into a second read of the same file.
    #[test]
    fn a_picture_put_in_front_is_still_only_read_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let count = 40usize;
        let mut wanted = Vec::new();
        for index in 0..count {
            let name = format!("{index}.png");
            write(&dir.path().join(&name), 300, 200);
            wanted.push((index as i64, name));
        }

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();
        thumbs.prime(
            dir.path(),
            wanted.iter().map(|(id, name)| (*id, name.as_str())),
            THUMB_EDGE,
        );
        let last = &wanted[count - 1];
        while !thumbs.pending.is_empty() {
            thumbs.collect(&ctx);
            thumbs.get(last.0, THUMB_EDGE, dir.path(), &last.1);
        }

        assert_eq!(thumbs.textures.len() + thumbs.ready.len(), count);
        assert_eq!(thumbs.tally.arrived, count as u64, "a picture was read twice");
    }

    /// Scrolling away from a tile before a worker has reached it takes it off
    /// them. What the eye is on now is read instead, and does not wait behind it.
    #[test]
    fn a_tile_that_scrolled_away_does_not_hold_up_the_one_on_screen() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("big.png"), 3000, 2000);
        write(&dir.path().join("small.png"), 300, 200);
        let big = (0i64, String::from("big.png"));
        let small = (1i64, String::from("small.png"));

        let ctx = egui::Context::default();
        let mut thumbs = Thumbnails::new();

        // The frame the big picture is on screen for, and the frame after it has
        // been scrolled past.
        thumbs.collect(&ctx);
        assert!(thumbs.get(big.0, THUMB_EDGE, dir.path(), &big.1).is_none());
        thumbs.collect(&ctx);
        assert!(thumbs.get(small.0, THUMB_EDGE, dir.path(), &small.1).is_none());

        let (took, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(&small));
        assert!(
            !thumbs.textures.contains_key(&(big.0, THUMB_EDGE)),
            "the picture on screen only arrived once the one scrolled past had, in {took:.2}s"
        );
    }

    /// Draw the given tiles frame after frame until every one of them has a
    /// picture, the way the window does: collect what has arrived, then ask for
    /// what is on screen and still missing.
    fn fill(
        thumbs: &mut Thumbnails,
        ctx: &egui::Context,
        root: &Path,
        on_screen: &[(i64, String)],
    ) -> (f64, u32) {
        let started = std::time::Instant::now();
        let mut frames = 0;
        loop {
            thumbs.collect(ctx);
            frames += 1;
            let missing = on_screen
                .iter()
                .filter(|(id, path)| thumbs.get(*id, THUMB_EDGE, root, path).is_none())
                .count();
            if missing == 0 {
                return (started.elapsed().as_secs_f64(), frames);
            }
        }
    }

    #[test]
    fn loading_something_that_is_not_an_image_gives_nothing_rather_than_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"just text").unwrap();
        assert!(load(&path, THUMB_EDGE).is_none());

        let broken = dir.path().join("bad.png");
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\ncut").unwrap();
        assert!(load(&broken, THUMB_EDGE).is_none());

        assert!(load(&dir.path().join("absent.png"), THUMB_EDGE).is_none());
    }

    #[test]
    fn a_small_image_is_not_enlarged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("small.png");
        write(&path, 40, 30);
        let image = load(&path, THUMB_EDGE).expect("decoded");
        assert_eq!(image.size, [40, 30]);

        let large = load(&path, LARGE_EDGE).expect("decoded");
        assert_eq!(large.size, [40, 30]);
    }
}
