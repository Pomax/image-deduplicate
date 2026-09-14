use super::*;

/// Every image filed under the value its hash takes in one band.
///
/// This is what replaces the join. The entries are laid out in band-value order,
/// so everything sharing a value is one run of them, and the images to compare
/// against are two array reads away rather than an index seek per row. Every
/// variant of every image is filed, and the lookup is made with the variant the
/// image was indexed under, which is what finds a rotated copy.
pub(super) struct BandIndex {
    /// Where each value's run begins, and one past the end for the last one.
    starts: Vec<u32>,
    /// Image positions, in band-value order.
    pub(super) entries: Vec<u32>,
}

/// Values one band can take. The band is 16 bits wide, so this array is the
/// lookup: there is nothing to search.
const BAND_VALUES: usize = 1 << fingerprint::BAND_BITS;

impl BandIndex {
    /// `of` is the images to file, by position. Only one image out of each set of
    /// identical ones is filed, so a folder holding a thousand copies of one
    /// picture costs one entry rather than a thousand that all collide.
    pub(super) fn build(images: &[Image], of: &[u32], band: usize) -> BandIndex {
        let mut starts = vec![0u32; BAND_VALUES + 1];
        for position in of {
            for variant in &images[*position as usize].bands {
                starts[variant[band] as usize + 1] += 1;
            }
        }
        for value in 1..starts.len() {
            starts[value] += starts[value - 1];
        }

        let mut cursor = starts.clone();
        let mut entries = vec![0u32; of.len() * fingerprint::VARIANTS];
        for position in of {
            for variant in &images[*position as usize].bands {
                let value = variant[band] as usize;
                entries[cursor[value] as usize] = *position;
                cursor[value] += 1;
            }
        }
        BandIndex { starts, entries }
    }

    pub(super) fn holders(&self, value: u16) -> &[u32] {
        let value = value as usize;
        &self.entries[self.starts[value] as usize..self.starts[value + 1] as usize]
    }
}

/// Every pair one band puts together, each pair once and in order.
pub(super) fn pairs_in_band(images: &[Image], of: &[u32], band: usize) -> Vec<(u32, u32)> {
    let index = BandIndex::build(images, of, band);
    let mut out: Vec<(u32, u32)> = Vec::new();
    for a in of {
        for &b in index.holders(images[*a as usize].bands[0][band]) {
            if b != *a {
                out.push(((*a).min(b), (*a).max(b)));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
