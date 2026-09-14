use super::*;

/// Everything the comparison looks at. Two images with the same one of these
/// answer the same way to every test the search makes, against every other image,
/// so only one of them has to be compared to anything.
///
/// The dimensions are part of it because the shape test is the one thing that can
/// separate two pictures with the same hash.
#[derive(PartialEq, Eq, Hash)]
struct Identity {
    variants: [Words; fingerprint::VARIANTS],
    ring: Vec<u32>,
    width: u32,
    height: u32,
    /// The folder it is in, when a search only matches inside one. Two copies of
    /// a picture in two folders are then two pictures, not one standing for the
    /// other: folded together they would come back as one set spanning both.
    folder: Option<String>,
}

impl Identity {
    fn of(image: &Image, within_a_folder: bool) -> Identity {
        Identity {
            variants: image.variants,
            ring: image.ring.iter().map(|value| value.to_bits()).collect(),
            width: image.width,
            height: image.height,
            folder: within_a_folder.then(|| folder_of(&image.rel_path).to_string()),
        }
    }
}

/// The folder a file is in, as the index writes paths: separated by forward
/// slashes, and relative to the folder that was scanned. Everything up to the
/// last of them, which is nothing at all for a file in the folder itself.
pub(super) fn folder_of(rel_path: &str) -> &str {
    match rel_path.rfind('/') {
        Some(at) => &rel_path[..at],
        None => "",
    }
}

/// The searches to run: one per folder when a search is held to folders, and
/// one holding everything when it is not.
///
/// In folder order, so a run reads the same way twice and a log of one can be
/// compared with a log of another.
pub(super) fn by_folder(images: &[Image], of: Vec<u32>, within_a_folder: bool) -> Vec<Vec<u32>> {
    if !within_a_folder {
        return vec![of];
    }
    let mut folders: std::collections::BTreeMap<&str, Vec<u32>> = std::collections::BTreeMap::new();
    for id in of {
        folders
            .entry(folder_of(&images[id as usize].rel_path))
            .or_default()
            .push(id);
    }
    folders.into_values().collect()
}

/// Group the images that are indistinguishable to the comparison.
///
/// One pass, one map: an identity that is already known gains a position, one
/// that is not starts a group with the position that found it. The first position
/// in each group is the one that goes through the pairing, and it stands for the
/// rest.
///
/// This is what keeps a folder holding many copies of one picture cheap. Those
/// copies all land in the same band bucket, and comparing a bucket is comparing
/// everything in it to everything else in it.
pub(super) fn fold_identical(images: &[Image], within_a_folder: bool) -> Vec<Vec<u32>> {
    let mut known: std::collections::HashMap<Identity, usize> =
        std::collections::HashMap::with_capacity(images.len());
    let mut groups: Vec<Vec<u32>> = Vec::with_capacity(images.len());
    for (position, image) in images.iter().enumerate() {
        match known.get(&Identity::of(image, within_a_folder)) {
            Some(group) => groups[*group].push(position as u32),
            None => {
                known.insert(Identity::of(image, within_a_folder), groups.len());
                groups.push(vec![position as u32]);
            }
        }
    }
    groups
}

/// Which images ended up connected to which. Matched pairs are edges and a set is
/// everything one of them can be walked to from, so a chain of near matches is
/// one set even where its ends do not match each other.
pub(super) struct Groups {
    parent: Vec<u32>,
}

impl Groups {
    pub(super) fn new(count: usize) -> Groups {
        Groups {
            parent: (0..count as u32).collect(),
        }
    }

    pub(super) fn root(&mut self, mut of: u32) -> u32 {
        while self.parent[of as usize] != of {
            let grandparent = self.parent[self.parent[of as usize] as usize];
            self.parent[of as usize] = grandparent;
            of = grandparent;
        }
        of
    }

    /// The lower position wins, so a set is named after its earliest image and the
    /// name does not depend on the order the edges arrived in.
    pub(super) fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.root(a), self.root(b));
        if a == b {
            return;
        }
        let (low, high) = (a.min(b), a.max(b));
        self.parent[high as usize] = low;
    }
}
