//! A feature fingerprint: what is in the picture, and where, rather than what
//! the whole frame averages out to.
//!
//! The perceptual hash beside this one squashes the entire frame onto one square
//! and describes that. It is exact about a picture that fills the frame the same
//! way and blind to a crop, because cropping stretches a different region over
//! the same square and every number changes at once.
//!
//! This finds corners in the picture, describes the neighbourhood of each one,
//! and stores those. A crop keeps the corners that are still in it, and they are
//! described the same way whatever else was cut off, so two pictures are the same
//! picture when enough of their corners agree and agree in the same arrangement.
//!
//! Every part of it looks at one picture on its own. Nothing is learned from a
//! folder, nothing is trained, and the fingerprint of a file does not depend on
//! what else was scanned with it.

use image::{imageops, GrayImage};

mod agreement;
mod corners;
mod describe;

pub use self::agreement::*;
use self::corners::*;
pub use self::describe::*;

/// Long edge the picture is decoded to before corners are looked for.
///
/// The hash needs 64 pixels and takes 128; corners need enough pixels to have a
/// neighbourhood worth describing. Half of a 512 pixel picture is still 256,
/// which holds up.
pub const FEATURE_EDGE: u32 = 512;

/// Corners kept per picture.
///
/// A landscape has thousands and the strongest of them spread over the frame is
/// what a crop keeps some of. How many to keep is a trade against the size of
/// the index: a third of a picture is a third of its corners, and of those only
/// the ones the crop also picked as its own strongest are found again, so a
/// budget of a hundred leaves a crop and its original agreeing on ten-odd
/// corners, which is inside the noise. Three hundred puts it comfortably clear
/// and costs eleven kilobytes a picture.
pub const KEYPOINTS: usize = 320;

/// Bits in one corner's description.
pub const DESCRIPTOR_BITS: usize = 256;
pub const DESCRIPTOR_BYTES: usize = DESCRIPTOR_BITS / 8;

/// Levels of the pyramid, each this much smaller than the one before it. A crop
/// enlarged to the same size as the original shows its corners at a different
/// size, and the level they are found on absorbs that.
///
/// The steps are close together on purpose: whatever the size difference between
/// two pictures, some level of one is within half a step of some level of the
/// other, and half of a small step is a small error. At 1.2 the worst case is a
/// tenth, which the descriptions below shrug off; at 1.4 it is a fifth, which
/// measured as the difference between finding a crop and missing it.
const LEVELS: usize = 8;
const LEVEL_SCALE: f32 = 1.2;

/// How much brighter or darker than the middle the arc has to be.
const FAST_THRESHOLD: i16 = 18;
/// Pixels of the sixteen around a corner that have to agree.
const FAST_ARC: usize = 9;
/// Radius of the patch a description is sampled from.
const PATCH: i32 = 15;

/// One corner: where it is in the picture, and what it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keypoint {
    /// Where the corner sits in the decoded picture, in pixels.
    pub x: u16,
    pub y: u16,
    pub descriptor: [u8; DESCRIPTOR_BYTES],
}

/// Bytes one stored corner occupies: where it is, then what it looks like.
pub const KEYPOINT_BYTES: usize = 4 + DESCRIPTOR_BYTES;

/// Find the corners of a picture and describe them.
pub fn features(picture: &GrayImage) -> Vec<Keypoint> {
    let mut found: Vec<(u32, Keypoint)> = Vec::new();
    let mut level = picture.clone();
    let mut scale = 1.0f32;

    for step in 0..LEVELS {
        if level.width() < 2 * PATCH as u32 || level.height() < 2 * PATCH as u32 {
            break;
        }
        // Corners are found on the picture as it is, and described from a
        // smoothed copy of it. A description is 256 comparisons of one pixel
        // against another, and single pixels differ between two copies of a
        // picture for reasons that have nothing to do with what is in it:
        // compression, resampling, the sensor. Smoothing first is what makes the
        // answers the same for both.
        let smoothed = smooth(&level);
        for (x, y, score) in corners(&level) {
            let Some(angle) = orientation(&level, x, y) else {
                continue;
            };
            let descriptor = describe(&smoothed, x, y, angle);
            let point = Keypoint {
                x: (x as f32 * scale).round() as u16,
                y: (y as f32 * scale).round() as u16,
                descriptor,
            };
            found.push((score, point));
        }
        if step + 1 == LEVELS {
            break;
        }
        let (width, height) = (
            (level.width() as f32 / LEVEL_SCALE) as u32,
            (level.height() as f32 / LEVEL_SCALE) as u32,
        );
        if width < 2 * PATCH as u32 || height < 2 * PATCH as u32 {
            break;
        }
        level = imageops::resize(&level, width, height, imageops::FilterType::Triangle);
        scale *= LEVEL_SCALE;
    }

    strongest(found, picture.width(), picture.height())
}

/// Average each pixel with the eight around it, twice, which is close enough to
/// a gaussian for this and costs four passes over the picture.
fn smooth(picture: &GrayImage) -> GrayImage {
    let once = box_blur(picture);
    box_blur(&once)
}

fn box_blur(picture: &GrayImage) -> GrayImage {
    let (width, height) = picture.dimensions();
    let across = GrayImage::from_fn(width, height, |x, y| {
        let left = picture.get_pixel(x.saturating_sub(1), y).0[0] as u16;
        let middle = picture.get_pixel(x, y).0[0] as u16;
        let right = picture.get_pixel((x + 1).min(width - 1), y).0[0] as u16;
        image::Luma([((left + middle + right) / 3) as u8])
    });
    GrayImage::from_fn(width, height, |x, y| {
        let up = across.get_pixel(x, y.saturating_sub(1)).0[0] as u16;
        let middle = across.get_pixel(x, y).0[0] as u16;
        let down = across.get_pixel(x, (y + 1).min(height - 1)).0[0] as u16;
        image::Luma([((up + middle + down) / 3) as u8])
    })
}

/// The strongest corners, spread over the picture.
///
/// Taking the strongest outright gives every one of them to whichever part of the
/// picture has the most texture, and a crop of any other part matches nothing.
/// The frame is divided into cells and each cell keeps its own best.
fn strongest(mut found: Vec<(u32, Keypoint)>, width: u32, height: u32) -> Vec<Keypoint> {
    const CELLS: u32 = 8;
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let (cell_w, cell_h) = ((width / CELLS).max(1), (height / CELLS).max(1));
    let per_cell = KEYPOINTS / (CELLS * CELLS) as usize + 1;

    let mut taken = std::collections::HashMap::<(u32, u32), usize>::new();
    let mut out = Vec::with_capacity(KEYPOINTS);
    for (_, point) in &found {
        if out.len() >= KEYPOINTS {
            break;
        }
        let cell = (point.x as u32 / cell_w, point.y as u32 / cell_h);
        let count = taken.entry(cell).or_insert(0);
        if *count >= per_cell {
            continue;
        }
        *count += 1;
        out.push(*point);
    }
    // A picture with texture in one corner and flat sky everywhere else fills its
    // cells and stops short of the budget. The rest of the strongest fill it.
    if out.len() < KEYPOINTS {
        for (_, point) in &found {
            if out.len() >= KEYPOINTS {
                break;
            }
            if !out.contains(point) {
                out.push(*point);
            }
        }
    }
    out
}

/// Pack corners for the index: position then description, one after another.
pub fn pack(points: &[Keypoint]) -> Vec<u8> {
    let mut out = Vec::with_capacity(points.len() * KEYPOINT_BYTES);
    for point in points {
        out.extend_from_slice(&point.x.to_le_bytes());
        out.extend_from_slice(&point.y.to_le_bytes());
        out.extend_from_slice(&point.descriptor);
    }
    out
}

pub fn unpack(bytes: &[u8]) -> Vec<Keypoint> {
    bytes
        .chunks_exact(KEYPOINT_BYTES)
        .map(|chunk| Keypoint {
            x: u16::from_le_bytes([chunk[0], chunk[1]]),
            y: u16::from_le_bytes([chunk[2], chunk[3]]),
            descriptor: chunk[4..].try_into().unwrap_or([0; DESCRIPTOR_BYTES]),
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/features.rs"]
mod tests;
