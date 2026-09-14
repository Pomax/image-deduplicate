use super::*;

/// What the workers read from.
pub(super) struct Lanes {
    pub(super) queues: Mutex<Queues>,
    pub(super) ready: Condvar,
}

/// One picture to read: which file, and which result it belongs to.
pub(super) struct Wanted {
    pub(super) key: Key,
    pub(super) path: PathBuf,
    pub(super) result: u64,
}

#[derive(Default)]
pub(super) struct Queues {
    /// The tiles the last frame drew, the first of them at the back.
    pub(super) wanted: Vec<Wanted>,
    pub(super) rest: VecDeque<Wanted>,
    /// Nothing from `rest` is started while a tile on screen is still missing.
    pub(super) hold_rest: bool,
    /// Keys a worker has taken. A picture can be in both lanes, and this is what
    /// stops it being decoded twice.
    pub(super) started: HashSet<Key>,
    /// Which result the queues are holding. A new one makes every key mean a
    /// different file, so anything from an older one is thrown away.
    pub(super) result: u64,
    pub(super) stop: bool,
}

impl Queues {
    /// What a worker that only ever reads the screen takes.
    pub(super) fn take_wanted(&mut self) -> Option<Wanted> {
        while let Some(next) = self.wanted.pop() {
            if self.started.insert(next.key) {
                return Some(next);
            }
        }
        None
    }

    /// What a worker that only ever reads the background takes. It takes nothing
    /// at all while a tile on screen is still missing.
    pub(super) fn take_rest(&mut self) -> Option<Wanted> {
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
pub(super) struct Tally {
    asked: u64,
    pub(super) arrived: u64,
    pub(super) failed: u64,
    decoding: f64,
    pub(super) uploading: f64,
    started: Option<std::time::Instant>,
    said: u64,
}

impl Tally {
    /// Every so many pictures, and once at the end.
    const EVERY: u64 = 200;

    pub(super) fn ask(&mut self) {
        if self.asked == 0 {
            self.started = Some(std::time::Instant::now());
        }
        self.asked += 1;
    }

    pub(super) fn arrive(&mut self, decoding: f64, uploading: f64, ok: bool) {
        self.arrived += 1;
        self.decoding += decoding;
        self.uploading += uploading;
        if !ok {
            self.failed += 1;
        }
    }

    pub(super) fn say(&mut self, force: bool) {
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
pub(super) const SCREEN_WORKERS: usize = 24;

/// Threads reading ahead. Four, because every one of them is a picture already
/// being decoded that a scroll has to wait behind.
pub(super) const BACKGROUND_WORKERS: usize = 4;

/// A file at one size. The grid and the pane beside it want different sizes of
/// the same picture, and those are different pictures as far as the texture is
/// concerned.
pub(super) type Key = (i64, u32);

pub(super) struct Decoded {
    pub(super) key: Key,
    pub(super) image: Option<ColorImage>,
    /// The result this was asked for. One from an older result is dropped.
    pub(super) result: u64,
    /// Seconds spent reading and decoding it.
    pub(super) took: f64,
}
