use super::*;

/// What came back from reading one file.
///
/// The two that are not pictures carry what the index records about them, so a
/// later pass and a later comparison know the file has been looked at and that
/// looking again would find what this found.
pub(super) enum Outcome {
    Indexed(Box<Record>),
    /// Not one of the supported formats, or animated. Not an error, and not indexed.
    NotAnImage(db::Looked),
    Failed {
        looked: db::Looked,
        message: String,
    },
}

/// Where a pass spends its time inside the files, added up over every thread.
/// The totals are larger than the wall clock, by roughly the number of threads.
/// They are only ever read by the run log, so a build without the log carries
/// the empty version below and none of the timing.
#[cfg(feature = "logging")]
#[derive(Default)]
pub(super) struct Spent {
    pub(super) reading: AtomicU64,
    pub(super) decoding: AtomicU64,
    pub(super) fingerprinting: AtomicU64,
}

#[cfg(feature = "logging")]
impl Spent {
    pub(super) fn add(counter: &AtomicU64, at: Instant) {
        counter.fetch_add(at.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    pub(super) fn seconds(counter: &AtomicU64) -> f64 {
        counter.load(Ordering::Relaxed) as f64 / 1e3
    }
}

#[cfg(not(feature = "logging"))]
#[derive(Default)]
pub(super) struct Spent;

#[cfg_attr(not(feature = "logging"), allow(unused_variables))]
pub(super) fn index_one(candidate: &Candidate, bytes: &[u8], spent: &Spent) -> Outcome {
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let Some(format) = format::detect(head) else {
        return Outcome::NotAnImage(looked_at(candidate));
    };
    if frames::is_animated(format, &bytes) {
        return Outcome::NotAnImage(looked_at(candidate));
    }

    #[cfg(feature = "logging")]
    let at = Instant::now();
    let ready = match decode_for_indexing(format, &bytes) {
        Ok(ready) => ready,
        Err(err) => {
            return Outcome::Failed {
                looked: looked_at(candidate),
                message: format!("{err:#}"),
            }
        }
    };
    let decoded = ready.decoded;
    #[cfg(feature = "logging")]
    Spent::add(&spent.decoding, at);
    crate::log_line!(
        "decoded {} as {format}, {}x{}, {} bytes on disk",
        candidate.rel_path,
        decoded.width,
        decoded.height,
        bytes.len()
    );

    #[cfg(feature = "logging")]
    let at = Instant::now();
    let print = fingerprint(&decoded);
    let corners = crate::features::pack(&crate::features::features(&ready.detail));
    // The size of the picture, not of the sensor read that produced it: a camera
    // held on its side writes a wide picture and a number saying to turn it, and
    // the tile beside the turned picture has to say what is on it.
    let (width, height) = match crate::preview::the_way_up(&bytes) {
        5..=8 => (decoded.height, decoded.width),
        _ => (decoded.width, decoded.height),
    };
    #[cfg(feature = "logging")]
    Spent::add(&spent.fingerprinting, at);

    Outcome::Indexed(Box::new(Record {
        rel_path: candidate.rel_path.clone(),
        size_bytes: candidate.size_bytes,
        mtime_seconds: candidate.mtime_seconds,
        width,
        height,
        format,
        channels: decoded.channels,
        fingerprint: print,
        corners,
    }))
}
