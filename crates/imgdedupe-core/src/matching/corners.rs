use super::*;

/// Corners of one picture the quick look tries, against all of the other's. The
/// strongest, because those are the ones another picture of the same thing also
/// picked out.
const QUICK_CORNERS: usize = 48;

/// How alike two descriptions have to be for the quick look to count them.
///
/// Tight, and that is the whole trick. Loosely alike means nothing: with three
/// hundred corners to choose from, almost every corner of an unrelated
/// photograph has one within a quarter of its bits somewhere, and measured that
/// way unrelated pictures scored forty-one out of forty-eight. Within a
/// twelfth of the bits, unrelated pictures scored three and a picture against a
/// crop of itself scored thirty-five.
const QUICK_BITS: u32 = 24;

/// Corners that have to look alike before a pair is worth the real test. Three
/// was the most any of a hundred and ninety unrelated pairs managed.
const QUICK_AGREEING: usize = 3;

/// Corners of a picture the index holds, strongest first. A picture asks with
/// its strongest few dozen and is answered from these.
const INDEXED_CORNERS: usize = 128;

/// Ways the index files one description.
///
/// A filing is a sample of the description's bits. Two descriptions differing
/// in a twelfth of their bits land under the same value about a quarter of the
/// time, so several filings put them together nine times in ten, which is the
/// banding of the hash index applied to a corner. Sampled bits rather than a
/// fixed slice, because a fixed slice is precisely what fails when the
/// difference happens to fall inside it.
const CORNER_TABLES: usize = 16;

/// Pictures the shortlist gets through between saying so. Often enough that the
/// bar moves on a small folder, rarely enough that saying so costs nothing.
const SHORTLIST_REPORT_EVERY: u64 = 16;

/// Corners a bucket is meant to hold, which is what one look at one table reads.
/// The number of buckets follows the folder to keep it there, so a folder ten
/// times the size is ten times the buckets and the same reading per picture.
/// That is what makes this pass grow with the folder rather than its square.
const BUCKET_TARGET: usize = 96;

/// Bits a table may sample, at fewest and at most. The floor is a small folder,
/// where buckets are nearly empty anyway; the ceiling is where the bucket ends
/// are more memory than the entries in them.
const FEWEST_TABLE_BITS: u32 = 6;
const MOST_TABLE_BITS: u32 = 20;

/// How many times the average a bucket may hold before it is skipped as saying
/// nothing. A description shared by that many corners does not tell one picture
/// from another, and reading those buckets was measured to be nineteen
/// twentieths of the pass and none of the answer.
const BUCKET_LIMIT: usize = 32;

/// Room in an entry for which corner of the picture it is, leaving the rest of
/// the entry for which picture. Seven bits is [`INDEXED_CORNERS`], and the
/// twenty five left over are more pictures than a folder can hold.
const CORNER_ROOM: u32 = 7;

const _: () = assert!(QUICK_CORNERS <= u64::BITS as usize);
const _: () = assert!(INDEXED_CORNERS <= 1 << CORNER_ROOM);

/// The bit positions each table samples, most it could need first. Fixed, so a
/// folder is filed the same way on every run and on every machine.
static CORNER_BITS: std::sync::LazyLock<[[u8; MOST_TABLE_BITS as usize]; CORNER_TABLES]> =
    std::sync::LazyLock::new(|| {
        let mut state = 0x2f6e_2b1d_9a37_c5e1_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let mut tables = [[0u8; MOST_TABLE_BITS as usize]; CORNER_TABLES];
        for table in tables.iter_mut() {
            let mut taken = [false; features::DESCRIPTOR_BITS];
            for place in table.iter_mut() {
                loop {
                    let bit = next() % features::DESCRIPTOR_BITS;
                    if !taken[bit] {
                        taken[bit] = true;
                        *place = bit as u8;
                        break;
                    }
                }
            }
        }
        tables
    });

/// The value a description files under in one table: its sampled bits, in order.
fn corner_key(descriptor: &[u8; features::DESCRIPTOR_BYTES], bits: &[u8]) -> usize {
    let mut key = 0;
    for (place, bit) in bits.iter().enumerate() {
        let set = descriptor[(bit / 8) as usize] >> (bit % 8) & 1;
        key |= (set as usize) << place;
    }
    key
}

/// Bits a table samples for a folder holding this many corners: enough buckets
/// to leave [`BUCKET_TARGET`] of them in each.
fn table_bits(entries: usize) -> u32 {
    let wanted = (entries / BUCKET_TARGET)
        .max(1)
        .next_power_of_two()
        .trailing_zeros();
    wanted.clamp(FEWEST_TABLE_BITS, MOST_TABLE_BITS)
}

/// One filing of every corner in the folder: buckets of entries, an entry being
/// a picture and which of its corners this is.
struct CornerTable {
    bits: Vec<u8>,
    /// Bucket `k` is `entries[starts[k]..starts[k + 1]]`.
    starts: Vec<u32>,
    entries: Vec<u32>,
}

impl CornerTable {
    fn of(images: &[Image], of: &[u32], sampling: &[u8], bits: u32) -> Self {
        let bits = sampling[..bits as usize].to_vec();
        let mut starts = vec![0u32; (1 << bits.len()) + 1];
        for id in of {
            for corner in images[*id as usize].corners.iter().take(INDEXED_CORNERS) {
                starts[corner_key(&corner.descriptor, &bits) + 1] += 1;
            }
        }
        for k in 1..starts.len() {
            starts[k] += starts[k - 1];
        }
        let mut filled = starts.clone();
        let mut entries = vec![0u32; *starts.last().unwrap_or(&0) as usize];
        for id in of {
            let corners = images[*id as usize].corners.iter().take(INDEXED_CORNERS);
            for (which, corner) in corners.enumerate() {
                let k = corner_key(&corner.descriptor, &bits);
                entries[filled[k] as usize] = (id << CORNER_ROOM) | which as u32;
                filled[k] += 1;
            }
        }
        CornerTable {
            bits,
            starts,
            entries,
        }
    }

    fn bucket(&self, descriptor: &[u8; features::DESCRIPTOR_BYTES]) -> &[u32] {
        let k = corner_key(descriptor, &self.bits);
        &self.entries[self.starts[k] as usize..self.starts[k + 1] as usize]
    }
}

/// Pairs of pictures worth comparing corner by corner.
///
/// A picture is worth comparing against another when several of its strongest
/// corners are described almost exactly as one of the other's is. Finding those
/// by looking at every pair is quadratic, and a folder of ten thousand pictures
/// is forty five million pairs, which is minutes of work for an answer about a
/// few hundred of them.
///
/// So the corners are filed instead. A description is a string of bits where
/// near means alike, which is what the hash index already exploits per picture:
/// file each corner under a sample of its bits, several samples over, and two
/// descriptions a few bits apart share a bucket in at least one filing. A
/// picture then reads a few hundred corners rather than the folder's three
/// million, and the pass stops growing with the square of the folder.
///
/// Nothing here is learned from the folder: the samples are fixed and a
/// picture's own corners are its own.
pub(super) fn pairs_by_corner(
    images: &[Image],
    of: &[u32],
    stopped: &(dyn Fn() -> bool + Sync),
    report: &(dyn Fn(Progress) + Sync),
    looked: &AtomicU64,
    to_look: u64,
) -> Vec<(u32, u32)> {
    // An entry is a picture and one of its corners in one number, which stops
    // being true at a folder of thirty three million pictures. Nothing else
    // here works at that size either.
    if images.len() >= 1 << (u32::BITS - CORNER_ROOM) {
        return Vec::new();
    }
    let filed: usize = of
        .iter()
        .map(|id| images[*id as usize].corners.len().min(INDEXED_CORNERS))
        .sum();
    let bits = table_bits(filed);
    let most_in_a_bucket = (filed >> bits).max(1) * BUCKET_LIMIT;
    let tables: Vec<CornerTable> = CORNER_BITS
        .par_iter()
        .map(|s| CornerTable::of(images, of, s, bits))
        .collect();
    crate::log_line!(
        "corner index: {} corners over {} tables of {} buckets, skipping past {}",
        filed,
        CORNER_TABLES,
        1 << bits,
        most_in_a_bucket
    );
    of.par_iter()
        .flat_map_iter(|id| {
            let mut agreeing: std::collections::HashMap<u32, u64> =
                std::collections::HashMap::new();
            let done = looked.fetch_add(1, Ordering::Relaxed) + 1;
            if done % SHORTLIST_REPORT_EVERY == 0 || done == to_look {
                report(Progress::Shortlisting {
                    done,
                    total: to_look,
                });
            }
            if !stopped() {
                let asking = images[*id as usize].corners.iter().take(QUICK_CORNERS);
                for (place, corner) in asking.enumerate() {
                    for table in &tables {
                        let bucket = table.bucket(&corner.descriptor);
                        // A description thousands of pictures share tells
                        // nothing about which picture this is. Reading such a
                        // bucket is most of the work and none of the answer:
                        // measured on ten thousand photographs, the fullest
                        // bucket held two percent of every corner in the folder
                        // and the pass spent nineteen twentieths of its time in
                        // buckets like it.
                        if bucket.len() > most_in_a_bucket {
                            continue;
                        }
                        for entry in bucket {
                            let other = entry >> CORNER_ROOM;
                            if other == *id {
                                continue;
                            }
                            let which = (entry & ((1 << CORNER_ROOM) - 1)) as usize;
                            let found = &images[other as usize].corners[which];
                            if features::distance(&corner.descriptor, &found.descriptor)
                                <= QUICK_BITS
                            {
                                *agreeing.entry(other).or_insert(0) |= 1 << place;
                            }
                        }
                    }
                }
            }
            let id = *id;
            agreeing
                .into_iter()
                .filter(|(_, alike)| alike.count_ones() as usize >= QUICK_AGREEING)
                .map(move |(other, _)| (id.min(other), id.max(other)))
        })
        .collect()
}
