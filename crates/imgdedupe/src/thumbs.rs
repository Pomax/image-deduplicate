use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{ColorImage, TextureHandle, TextureOptions};
use imgdedupe_core::decode::decode_at_most;
use imgdedupe_core::format::{self, SNIFF_LEN};

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
/// Every thumbnail is read at once, on several threads, as soon as the sets are
/// known, and everything read stays in memory. Scrolling never waits on a disk,
/// and neither does going back to a picture that was looked at before.
///
/// `egui` redraws every frame, so decoding on the frame that needs an image would
/// stall the window. Requests go to the workers and the grid shows a placeholder
/// until the answer arrives.
pub struct Thumbnails {
    textures: HashMap<Key, TextureHandle>,
    pending: HashSet<Key>,
    requests: Sender<Request>,
    results: Receiver<Decoded>,
}

/// A file at one size. The grid and the pane beside it want different sizes of
/// the same picture, and they are different pictures as far as the texture is
/// concerned.
type Key = (i64, u32);

struct Request {
    key: Key,
    path: PathBuf,
}

struct Decoded {
    key: Key,
    image: Option<ColorImage>,
}

impl Thumbnails {
    pub fn new() -> Self {
        let (requests, request_rx) = mpsc::channel::<Request>();
        let (result_tx, results) = mpsc::channel::<Decoded>();

        // Decoding is what takes the time, and there is a core per picture to
        // spare while someone is looking at the list.
        let workers = std::thread::available_parallelism().map_or(4, |count| count.get().min(8));
        let queue = Arc::new(Mutex::new(request_rx));
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let result_tx = result_tx.clone();
            std::thread::spawn(move || loop {
                let Ok(request) = ({
                    let queue = queue.lock().expect("the request queue");
                    queue.recv()
                }) else {
                    return;
                };
                let image = load(&request.path, request.key.1);
                if result_tx.send(Decoded { key: request.key, image }).is_err() {
                    return;
                }
            });
        }

        Thumbnails {
            textures: HashMap::new(),
            pending: HashSet::new(),
            requests,
            results,
        }
    }

    /// Take everything the workers have finished. Called once per frame.
    pub fn collect(&mut self, ctx: &egui::Context) {
        while let Ok(decoded) = self.results.try_recv() {
            self.pending.remove(&decoded.key);
            let Some(image) = decoded.image else {
                continue;
            };
            let (file_id, edge) = decoded.key;
            let handle = ctx.load_texture(
                format!("preview{edge}-{file_id}"),
                image,
                TextureOptions::default(),
            );
            self.textures.insert(decoded.key, handle);
        }

        // Nothing else will wake the window when a picture finishes reading, and
        // then it appears on whatever frame some other input happens to cause.
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
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
        self.request(key, root, rel_path);
        None
    }

    /// Ask for every picture in the list up front, rather than when it is
    /// scrolled to. Called once, when the sets are found.
    pub fn prime<'a>(
        &mut self,
        root: &Path,
        members: impl Iterator<Item = (i64, &'a str)>,
        edge: u32,
    ) {
        for (file_id, rel_path) in members {
            self.request((file_id, edge), root, rel_path);
        }
    }

    fn request(&mut self, key: Key, root: &Path, rel_path: &str) {
        if self.textures.contains_key(&key) || self.pending.contains(&key) {
            return;
        }
        self.pending.insert(key);
        let _ = self.requests.send(Request { key, path: root.join(rel_path) });
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
    let size = [decoded.small.width() as usize, decoded.small.height() as usize];
    Some(ColorImage::from_rgb(size, decoded.small.as_raw()))
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

        let mut thumbs = Thumbnails::new();
        thumbs.prime(
            dir.path(),
            wanted.iter().map(|(id, name)| (*id, name.as_str())),
            THUMB_EDGE,
        );

        let mut arrived = 0;
        while arrived < wanted.len() {
            let decoded = thumbs.results.recv().expect("a decoded picture");
            assert!(decoded.image.is_some(), "{:?} did not decode", decoded.key);
            arrived += 1;
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
