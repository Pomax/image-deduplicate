use super::*;

/// Rings are rotation invariant only inside the largest circle that fits in the
/// image. Past that radius a ring is clipped by the long edges, and a 90 degree
/// rotation clips it on the other axis, so the two disagree. Sampling stops at
/// `min(w, h) / 2` and the corners are simply not looked at.
pub(super) fn ring_stats(small: &RgbImage) -> Vec<u8> {
    let (w, h) = (small.width() as f32, small.height() as f32);
    let (cx, cy) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    let radius = (w.min(h) / 2.0).max(1.0);

    let mut sums = [[0.0f64; 4]; RINGS];
    let mut counts = [0u32; RINGS];

    for (x, y, pixel) in small.enumerate_pixels() {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance > radius {
            continue;
        }
        let ring = ((distance / radius) * RINGS as f32) as usize;
        let ring = ring.min(RINGS - 1);
        let (l, a, b) = oklab(pixel.0);
        sums[ring][0] += l as f64;
        sums[ring][1] += a as f64;
        sums[ring][2] += b as f64;
        sums[ring][3] += (l * l) as f64;
        counts[ring] += 1;
    }

    let mut out = Vec::with_capacity(RINGS * RING_VALUES * 4);
    for ring in 0..RINGS {
        let (mean_l, mean_a, mean_b, sd_l) = if counts[ring] == 0 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let n = counts[ring] as f64;
            let mean_l = sums[ring][0] / n;
            let variance = (sums[ring][3] / n - mean_l * mean_l).max(0.0);
            (
                mean_l as f32,
                (sums[ring][1] / n) as f32,
                (sums[ring][2] / n) as f32,
                variance.sqrt() as f32,
            )
        };
        for value in [mean_l, mean_a, mean_b, sd_l] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// sRGB to Oklab. The transfer function is a 256-entry table because there are
/// only 256 possible inputs; the three cube roots that follow are what this costs
/// and are why it is called once per pixel and not once per statistic.
fn oklab(rgb: [u8; 3]) -> (f32, f32, f32) {
    let linear = linear_table();
    let (r, g, b) = (
        linear[rgb[0] as usize],
        linear[rgb[1] as usize],
        linear[rgb[2] as usize],
    );

    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();

    (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

fn linear_table() -> &'static [f32; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0f32; 256];
        for (value, slot) in table.iter_mut().enumerate() {
            let c = value as f32 / 255.0;
            *slot = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        table
    })
}
