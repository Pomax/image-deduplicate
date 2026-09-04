# imgdedupe

A cross-platform desktop program that indexes a folder of images, finds duplicates in it, and removes the ones you don't keep. Written in Rust with egui. Builds to one executable, with no installer and no runtime dependencies.

## Supported image formats:

- JPEG
- PNG
- GIF
- WebP

## The three steps

Each step has its own "page" in the UI.

**Scan.** Walks the folder (subfolders optional), decodes and fingerprints every file that is new or has changed, and writes the results to `imgdedupe.sqlite` in that folder. Files whose size and modification time match the index are skipped.

**Review.** Shows each duplicate set as a row of thumbnails with one marked as the keeper. Click to preview, double click or space to change the keeper, arrow keys to move, "keep all" and "keep none" per set.

**Clean up.** Removes everything not marked as kept, to the recycle bin, to a folder that mirrors the original structure, or by deleting. Failures are listed with their reason.

Dropping a folder onto the window does the first two steps: it opens the folder, scans it, and searches it.

## How duplicates are found

**Decode.** Each file is decoded to at most 128px on its long edge. JPEG uses [jpeg-decoder](https://github.com/image-rs/jpeg-decoder)'s DCT-scaled decode, which decodes at 1/2, 1/4 or 1/8 scale directly from the coefficients instead of decoding in full and downsampling.

**Perceptual hash.** The decoded image is converted to [Rec. 709](https://www.itu.int/rec/R-REC-BT.709) luma, resampled to a fixed 64x64 square (which is what makes the hash independent of scale and aspect ratio), and run through a 2D [DCT](https://en.wikipedia.org/wiki/Discrete_cosine_transform). The top-left 16x16 block of coefficients minus the DC term gives 255 bits, set by comparing each coefficient against the median of the block.

This is the DCT hash described in Zauner's [Implementation and Benchmarking of Perceptual Image Hash Functions](https://www.phash.org/docs/pubs/thesis_zauner.pdf) and in Krawetz's [Looks Like It](https://www.hackerfactor.com/blog/index.php?/archives/432-Looks-Like-It.html), with a 16x16 coefficient block rather than the usual 8x8, and a median threshold rather than a mean.

**Rotation and mirroring.** All eight symmetries of the square are hashed and stored, rather than deriving a canonical orientation. Resampling a rotated image onto the square produces the rotation of the resampled square, so the eight hashes are exact, and a rotated copy matches without a per-image orientation decision that resizing could flip.

**Colour signature.** 12 concentric rings inside the inscribed circle, each contributing mean L, mean a, mean b and the standard deviation of L in [Oklab](https://bottosson.github.io/posts/oklab/). Rings are rotation invariant by construction. This is what separates a colourised copy from its grayscale original, and the "match colour with grayscale" checkbox skips the check.

**Candidate selection.** Comparing every pair is quadratic and does not scale. Each hash is cut into 16-bit bands and only images sharing a band are compared, which is the banding scheme used for [locality-sensitive hashing](https://en.wikipedia.org/wiki/Locality-sensitive_hashing) in chapter 3 of [Mining of Massive Datasets](http://www.mmds.org/). Two hashes differing by fewer bits than there are bands must agree on at least one band, so everything within that radius is guaranteed to be found; in practice the shortlist reaches considerably further, because scattered differing bits tend to leave some band untouched. Bands are computed while loading the index, not stored.

**Verification.** Each candidate pair is compared properly: [Hamming distance](https://en.wikipedia.org/wiki/Hamming_distance) against all eight variants of the other image, taking the closest, then the ring distance unless colour is being ignored. Surviving pairs are merged into sets with a [disjoint-set structure](https://en.wikipedia.org/wiki/Disjoint-set_data_structure), with byte-identical images folded together first so a hundred copies of one file compare as one.

**Threshold.** The sensitivity slider is a percentage of the hash length, so the numbers keep their meaning if the hash length changes. Measured: resizes, recompressions and rotations of one picture move the hash by up to about 8%, unrelated images sit above 25%. Presets are close (5%), balanced (15%, the default), wide (30%) and yolo (50%).

**Keeper selection.** Scored per image: log2 of the pixel count dominates, then a budget smaller than one resolution doubling covering lossless format, bytes per pixel, colour, alpha, and filename or folder markers that indicate a copy (`- Copy`, `(1)`, `/downloads/`).

## Concurrency

Decoding and fingerprinting run on all cores through rayon; results go down a channel to a single writer thread that commits in transactions of 5000 records. Comparison is split across cores in batches. Scan, search and cleanup each run off the UI thread and report progress through channels the frame loop drains.

Thumbnails use two worker pools: up to 24 threads serving what is currently on screen, and 4 reading ahead, with the read-ahead pool held while anything on screen is still missing. Decoded images become GPU textures on the frame that draws them.

## The index

`imgdedupe.sqlite` in the scanned folder: tables for files (path, size, mtime), images (dimensions, format, channels), fingerprints (packed hashes, ring stats), and a meta table holding the schema version, last scan time, cleanup destination and recursion flag. WAL mode, checkpointed and switched back to DELETE on close, so no `-wal` or `-shm` files are left behind.

The "Save an index database for this folder" checkbox controls whether that file is kept. Unticking it deletes the index immediately; a cleanup on an unticked folder deletes it when finished. The checkbox state comes from whether the folder contains an index.

## Building

Requires [Rust](https://rustup.rs).

- `build.bat` on Windows, `build.sh` elsewhere

Output goes to `dist/<platform>/`, with a copy in the repository root for immediate use. Release builds use fat LTO, one codegen unit, stripped symbols, `panic = "abort"`, and `opt-level = "z"` for the toolkit crates.

On Windows and Linux the result is compressed with UPX if installed. UPX-style binary compression is disallowed by MacOS and so not used on that platform.

## Tests

`test.bat` or `test.sh`. Arguments are passed to cargo, so a single test can be run by name. [tests.md](./tests.md) documents every test.

You can build a special test binary by using `--test` as argument to the build script, which yields a binary that lets you run it with the `--log` flag, which will log a stupid amount of information in order to assist in debugging.

## Questions and comments

If you found a bug, fixed one, or want it to do something it doesn't, file an issue over on https://github.com/Pomax/image-deduplicate

For less serious engagement, https://mastodon.social/@TheRealPomax
