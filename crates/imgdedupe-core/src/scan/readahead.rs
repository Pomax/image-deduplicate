use super::*;

/// How many bytes of already-read files may be waiting to be decoded when the
/// machine will not say how much memory it has.
pub(super) const FALLBACK_READ_AHEAD: u64 = 1 << 30;

/// How long an answer about available memory is used before it is asked for
/// again, and how long a waiting reader sleeps before looking at the budget on
/// its own.
pub(super) const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(250);

/// Bytes of read-but-not-yet-decoded files, and the wait for room.
pub(super) struct ReadAhead {
    pub(super) held: std::sync::Mutex<u64>,
    room: std::sync::Condvar,
    /// How much memory the machine has to spare.
    available: Box<dyn Fn() -> Option<u64> + Send + Sync>,
    last: std::sync::Mutex<Option<(std::time::Instant, Option<u64>)>>,
}

impl ReadAhead {
    pub(super) fn new() -> Self {
        Self::asking(Box::new(crate::memory::available_bytes))
    }

    pub(super) fn asking(available: Box<dyn Fn() -> Option<u64> + Send + Sync>) -> Self {
        ReadAhead {
            held: std::sync::Mutex::new(0),
            room: std::sync::Condvar::new(),
            available,
            last: std::sync::Mutex::new(None),
        }
    }

    /// How much may be in hand: nine tenths of what the machine has to spare
    /// plus what is already held, which the machine does not count as available.
    pub(super) fn budget(&self, held: u64) -> u64 {
        match self.available_now() {
            Some(spare) => spare.saturating_add(held) / 10 * 9,
            None => FALLBACK_READ_AHEAD,
        }
    }

    /// The last answer, asked again when it is older than `LOOK_AGAIN`.
    fn available_now(&self) -> Option<u64> {
        let mut last = self.last.lock().expect("the read-ahead budget");
        if let Some((asked, answer)) = *last {
            if asked.elapsed() < LOOK_AGAIN {
                return answer;
            }
        }
        let answer = (self.available)();
        *last = Some((std::time::Instant::now(), answer));
        answer
    }

    /// Wait until this many bytes fit, then claim them. A single file larger than
    /// the whole budget is let through on its own rather than waiting for room
    /// that will never exist. The wait is timed: room also appears when
    /// something else on the machine gives memory back, which nothing announces.
    pub(super) fn claim(&self, bytes: u64, cancel: &AtomicBool) {
        let mut held = self.held.lock().expect("the read-ahead budget");
        while *held > 0 && *held + bytes > self.budget(*held) && !cancel.load(Ordering::Relaxed) {
            let (next, _) = self
                .room
                .wait_timeout(held, LOOK_AGAIN)
                .expect("the read-ahead budget");
            held = next;
        }
        *held += bytes;
    }

    pub(super) fn release(&self, bytes: u64) {
        let mut held = self.held.lock().expect("the read-ahead budget");
        *held = held.saturating_sub(bytes);
        self.room.notify_all();
    }
}
