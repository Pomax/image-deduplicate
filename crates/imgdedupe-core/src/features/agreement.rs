use super::*;

/// The most bits two descriptions may differ in and still be the same corner.
/// A quarter of them: measured, corners of the same place in two copies of a
/// picture land under 60 and corners of different places sit around 128, which
/// is what two unrelated strings of bits do.
const SAME_CORNER: u32 = 64;

/// How much better the best match has to be than the second best, in tenths.
/// A corner that matches two places equally well matches neither: repeated
/// texture produces exactly that, and it is the usual way a match is wrong.
const CLEARLY_BETTER: u32 = 8;

/// Pixels a corner may sit away from where the arrangement says it should be.
const ROOM: f32 = 6.0;

/// Corners that have to agree, in the same arrangement, before two pictures are
/// the same picture.
///
/// Measured on photographs: 190 pairs of unrelated ones never got past ten and
/// mostly sat at zero, a picture and a sixty percent crop of it agreed on
/// thirty-seven, and frames of the same scene seconds apart on sixty to two
/// hundred. Sixteen sits between the two with room either side.
pub const AGREEING_CORNERS: u32 = 16;

/// How many corners of one picture are corners of the other, in the same
/// arrangement.
///
/// Corners are paired by description alone, which pairs some of them wrongly:
/// two windows of the same building look identical. What is left is geometry.
/// One scale, one rotation and one shift take the whole of a picture onto the
/// whole of a copy of it, or onto the part of it a crop kept, so a pair of
/// matches proposes such an arrangement and every other match votes on it. The
/// arrangement with the most votes is the answer, and the votes are the number
/// returned. Wrong pairs vote for nothing in particular and are left out.
pub fn agreement(one: &[Keypoint], other: &[Keypoint]) -> u32 {
    let pairs = paired(one, other);
    if pairs.len() < AGREEING_CORNERS as usize {
        return 0;
    }

    let mut best = 0;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..ATTEMPTS {
        let first = (random() as usize) % pairs.len();
        let second = (random() as usize) % pairs.len();
        if first == second {
            continue;
        }
        let Some(arrangement) = Arrangement::through(pairs[first], pairs[second]) else {
            continue;
        };
        let votes = pairs
            .iter()
            .filter(|pair| arrangement.holds(**pair))
            .count() as u32;
        best = best.max(votes);
        // Nothing is going to beat every pair agreeing.
        if best as usize == pairs.len() {
            break;
        }
    }
    best
}

/// Arrangements tried. Two matches out of a hundred-odd, drawn at random: a
/// hundred draws finds one that is right long before it runs out.
const ATTEMPTS: usize = 200;

/// One corner of each picture, paired because they describe the same thing.
type Pair = ((f32, f32), (f32, f32));

pub(super) fn paired(one: &[Keypoint], other: &[Keypoint]) -> Vec<Pair> {
    // Each corner's best in the other picture, in both directions. Both,
    // because a corner of one picture being another's best is not the same
    // thing as the other way round: line art and repeated texture give a
    // dozen corners of one picture the same best corner in the other, and a
    // dozen pairs that all end at one place is not two pictures arranged the
    // same way. It measured as exactly that: sixteen corners of one picture
    // agreeing on an arrangement that put all of them on two places in the
    // other, which is what let unrelated drawings reach the threshold.
    let mut best_of_other = vec![(u32::MAX, usize::MAX); other.len()];
    let mut best_of_one = vec![(u32::MAX, u32::MAX, usize::MAX); one.len()];
    for (which, point) in one.iter().enumerate() {
        for (candidate, held) in other.iter().enumerate() {
            let bits = distance(&point.descriptor, &held.descriptor);
            let (best, second, at) = &mut best_of_one[which];
            if bits < *best {
                *second = *best;
                *best = bits;
                *at = candidate;
            } else if bits < *second {
                *second = bits;
            }
            if bits < best_of_other[candidate].0 {
                best_of_other[candidate] = (bits, which);
            }
        }
    }

    let mut out = Vec::new();
    for (which, (best, second, at)) in best_of_one.into_iter().enumerate() {
        if at == usize::MAX {
            continue;
        }
        // Saturating, because a picture with one corner in it has no runner-up
        // to be clearly better than, and nothing to multiply.
        if best > SAME_CORNER || best.saturating_mul(10) > second.saturating_mul(CLEARLY_BETTER) {
            continue;
        }
        // Each corner pairs with the corner that also picked it.
        if best_of_other[at].1 != which {
            continue;
        }
        out.push((
            (one[which].x as f32, one[which].y as f32),
            (other[at].x as f32, other[at].y as f32),
        ));
    }
    out
}

/// A scale, a turn and a shift: what takes one picture onto another.
#[derive(Debug, Clone, Copy)]
struct Arrangement {
    /// The scale and the turn together, as one complex number.
    turn: (f32, f32),
    shift: (f32, f32),
}

impl Arrangement {
    /// The arrangement two pairs of corners propose. `None` when they propose
    /// nothing: two corners in the same place say nothing about scale or turn,
    /// and a scale far from one is not a crop of anything.
    fn through(first: Pair, second: Pair) -> Option<Arrangement> {
        let from = (second.0 .0 - first.0 .0, second.0 .1 - first.0 .1);
        let to = (second.1 .0 - first.1 .0, second.1 .1 - first.1 .1);
        let length = from.0 * from.0 + from.1 * from.1;
        if length < 16.0 {
            return None;
        }
        // to / from, as complex numbers: the scale and the turn in one.
        let turn = (
            (to.0 * from.0 + to.1 * from.1) / length,
            (to.1 * from.0 - to.0 * from.1) / length,
        );
        let scale = (turn.0 * turn.0 + turn.1 * turn.1).sqrt();
        if !(0.2..=5.0).contains(&scale) {
            return None;
        }
        let placed = apply(turn, first.0);
        Some(Arrangement {
            turn,
            shift: (first.1 .0 - placed.0, first.1 .1 - placed.1),
        })
    }

    fn holds(&self, pair: Pair) -> bool {
        let placed = apply(self.turn, pair.0);
        let (dx, dy) = (
            placed.0 + self.shift.0 - pair.1 .0,
            placed.1 + self.shift.1 - pair.1 .1,
        );
        dx * dx + dy * dy <= ROOM * ROOM
    }
}

fn apply(turn: (f32, f32), point: (f32, f32)) -> (f32, f32) {
    (
        turn.0 * point.0 - turn.1 * point.1,
        turn.1 * point.0 + turn.0 * point.1,
    )
}
