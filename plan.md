# Image dedupe: workplan

## The language choice

Start from what the program spends its time on, not from a preference.

For each file: read it, produce a small downscaled version of the picture, compute a few fingerprints from it, write a row to SQLite. At tens of thousands of files the decode dominates, and the fingerprint is the next largest cost, not a rounding error. Measured, single threaded, on a 1600px corpus: 779ms of decode against 306ms of fingerprinting for 304 JPEGs, with reading and hashing together under 50ms.

The language question is therefore four questions:

1. Can it decode as fast as the best decoder available for these formats?
2. Can it run the decodes on every core at once, with real threads?
3. Does it give a GUI that stays responsive over a list of thousands of thumbnails?
4. What does shipping it to Windows, macOS and Linux cost?

Question 1 depends entirely on the format set. For JPEG, PNG, GIF, WebP, TIFF, BMP and SVG there are mature pure-Rust decoders that are competitive with the C libraries, and one of them, `jpeg-decoder`, implements DCT-scaled decode, which is the largest single saving in the whole program. So this set can be decoded without linking any C at all. That was not true when HEIC, AVIF and JPEG XL were in scope: those exist only as C libraries, and their removal is what makes a self-contained binary possible again.

### Against the candidates

- **Python.** The GIL forces multiprocessing: one process per core, and pixel data crossing process boundaries. Its fast decoders (Pillow-SIMD, pyvips) are C libraries with the concurrency bolted on outside. A PyInstaller GUI executable on three platforms needs continuous maintenance. Rejected on threading and distribution.
- **Node.** `sharp` wraps libvips and decodes fast, but the GUI is Electron: hundreds of megabytes and a browser process to show a grid of thumbnails, and getting thumbnail blobs from SQLite into the renderer costs either base64 inflation or a custom protocol handler. Rejected on the GUI.
- **Go.** Good threading. `image/jpeg` in the standard library is several times slower than libjpeg-turbo and has no scaled decode, so it needs cgo to `govips`, which removes Go's build advantage. The GUI options are worse: Fyne does not virtualise large lists well, and Wails is Electron again with a different webview. Rejected on decode and GUI.
- **C#/.NET.** Real threads, NetVips is well maintained, and Avalonia is a better toolkit than egui for a data-heavy list, with real virtualisation and image caching built in. It loses on decode independence, since it must bind a C library for every format, and on packaging: NativeAOT plus Avalonia plus a chain of native image libraries is fragile, and macOS bundling and signing is worse than the alternatives.
- **C++.** The most direct: call libjpeg-turbo, libpng, libwebp and libtiff with no binding layer, and Qt is the best toolkit here by some distance. It loses on build and dependency management across three platforms, on Qt licensing complicating distribution, and on manual memory management in threaded code, which is where this kind of program develops defects.
- **Rust.** Decodes at the same speed with no C linked, because the pure-Rust decoders for this set are competitive and `jpeg-decoder` has the scaled path that matters most. `rayon` parallelises the file list without writing thread code. `rusqlite` compiles SQLite in. The build is `cargo build` on all three platforms and the result is one self-contained executable.

**Rust.** It is the only candidate that decodes this format set at full speed without linking a C library, which is what makes the single-file distribution real rather than aspirational.

The cost, stated plainly: `egui` is the weakest GUI toolkit of the three serious candidates. It needs an explicit thumbnail texture cache and manual row virtualisation through `ScrollArea::show_rows`, both of which Qt and Avalonia provide. That is a few hundred lines of work that the other two would not need.

### The crates

| Concern | Crate | Note |
| --- | --- | --- |
| Directory walk | `ignore` (or `jwalk`) | parallel walker, respects skip rules |
| JPEG | `jpeg-decoder` | `scale()`: DCT-scaled decode at 1/8, 1/4, 1/2 |
| PNG, GIF, BMP | `image` | pure Rust, `png` uses `fdeflate` |
| TIFF | `tiff` | reaches the reduced-resolution IFDs directly |
| WebP | `image-webp` | pure Rust, no scaled decode, see below |
| SVG | `resvg` | renders straight to the target size |
| Format detection | own magic-byte sniffing | only the formats in the set, everything else is skipped |
| Parallelism | `rayon` | |
| Database | `rusqlite` (`bundled`) | no system SQLite dependency |
| Byte hash | `xxhash-rust` (`xxh3`) | XXH3 truncated to 32 bits, several GB/s per core |
| GUI | `eframe` / `egui` | plus a thumbnail texture cache and row virtualisation |
| Deletion | `trash` | OS recycle bin, not `unlink` |
| CLI | `clap` | headless mode for scripting and benchmarks |

Every one of these is pure Rust. The build is `cargo build` with no system libraries, and the output is one executable per platform.

## What counts as an image

An image is a file holding a single still picture that a person looked at. That definition, not any library's capability list, decides the format set:

| Format | Extensions | Why it is in |
| --- | --- | --- |
| JPEG | jpg, jpeg, jpe, jfif | the majority of any photo folder |
| PNG | png | screenshots, saved web images, anything with transparency |
| WebP | webp | what a browser saves now |
| GIF | gif | still on the web and in saved-image folders |
| TIFF | tif, tiff | scanners and photo archives |
| BMP | bmp | old Windows output, still turns up |
| SVG | svg | a still picture people keep in image folders |

Excluded, with the reason:

- HEIC, AVIF, JPEG XL: the decoders exist only as C libraries (libheif with libde265, dav1d, libjxl), so supporting them means 30 to 60 MB of native libraries shipped alongside the executable and a per-platform build for each. Not worth it for their share of a folder.
- Camera RAW (CR2, NEF, ARW, DNG, RAF, ORF, RW2): sensor readings, not a picture. Nobody looked at the file, they looked at a rendering of it.
- Project files (PSD, XCF, AI, EPS, PDF): documents that contain images.
- Render and texture data (HDR, OpenEXR, DDS, KTX, TGA): pipeline data.
- Icon containers (ICO, ICNS): several sizes of one graphic in one file.
- PNM, PBM, PGM, PPM: a family name, and not a format found in image folders.
- QOI, farbfeld: no real-world corpus.
- Anything with more than one frame: animated GIF, APNG, animated WebP. Those are video in an image container. The frame count is in the header of all three, so rejecting them costs a header read and no decode.

Format is decided by reading the file's magic bytes, never the extension. Extensions are wrong often enough across tens of thousands of files that trusting them causes both failures and mismatches. Files that are not in the set are ignored and not recorded.

## The speed argument, concretely

Most of the runtime is decode, and most of that decode is unnecessary: the fingerprints need a 32x32 grayscale matrix and a colour histogram, not a 24 megapixel bitmap. So the centre of the design is one function, `decode_at_most(path, target_px)`, which returns the smallest image a format can produce without reconstructing the full one, with one implementation per format behind it. Nothing after that stage knows the format.

How much that function saves differs by format, and that is what decides the runtime:

| Format | Fast path | Saving |
| --- | --- | --- |
| SVG | render directly at the target size | total, there is no full-size stage |
| JPEG | DCT-scaled decode at 1/8 | around 1.5x, and a two hundredth of the memory |
| TIFF | the reduced-resolution IFDs, where the file has a pyramid | large when present, none otherwise |
| PNG | none, DEFLATE cannot be decoded partially | none, but streamed |
| WebP | none in the pure-Rust decoder | none |
| GIF | none | none |
| BMP | none, but it decodes fast anyway | none |

Three consequences:

- **The JPEG fast path is worth far less than it looks.** An earlier version of this plan claimed 5x to 10x, reasoning from output pixels: an eighth-scale reconstruction produces 64 times fewer of them. Measured against `zune-jpeg` decoding in full, over the whole path including the reduction that follows, it is 1.54x on a 2400x1800 file. Entropy decoding the bitstream cannot be skipped and is most of the cost; what the scaled path skips is the inverse DCT, the chroma upsampling and the colour conversion. It is still the right choice, and the second reason matters more than the first: it allocates a 300x225 buffer instead of a 2400x1800 one, and there is one of those per core.
- **PNG is the slow case.** DEFLATE cannot be decoded partially to a smaller image, so a large PNG always costs a full inflate and unfilter. The only savings available are decoding row by row and downsampling each row as it arrives, so the full-size buffer is never allocated, and reading only Adam7 pass 1 on the rare interlaced PNG. Measured at 678ms for 136 PNGs against 779ms for 304 JPEGs of similar total pixels, so roughly twice the cost per pixel.
- **WebP gives up its fast path.** libwebp can downscale during reconstruction and the pure-Rust `image-webp` cannot, so WebP is decoded in full and shrunk after. That is the price of not linking C. If a real folder shows WebP dominating the runtime, the fix is to link libwebp for that one format, which is a small self-contained dependency, not the whole chain that HEIC and AVIF would have required.

### What it actually does

Measured on a generated corpus, on a 32-core machine. A generated corpus is not a photo library, so these are indicative.

| | |
| --- | --- |
| One thread, 1600px images, all stages | 187 files/s, 212 megapixels/s |
| All cores, 550 files at 1600px | 0.5s |
| All cores, 27,500 files at 220px | 62s |
| Re-scan of an unchanged folder, 27,500 files | 0.1s |
| Matching, 27,500 files | 41s |
| Index size, 27,500 files | 128 MB, so 4.7 KB per image |

The re-scan figure is the one the incremental design exists for, and it is a directory walk and one query.

Indexing is dominated by the database, not by the pictures: the compute above is 187 files/s on one thread, which across cores is a few seconds of the 62, and the rest is writing 3.5 million band rows. Those rows are also most of the index size. `phash_bands` is `WITHOUT ROWID`, which was measured against a plain rowid table with the same two indexes and was 10 percent faster to fill, two and a half times faster to query, and a third smaller.

The rest of the pipeline does not depend on format:

- Files are hashed with XXH3 while their bytes are already in the page cache from the decode read, so exact-duplicate detection is nearly free.
- `rayon` over the file list, one SQLite writer thread fed by a channel, all inserts in batched transactions with WAL and `synchronous = NORMAL`.
- Re-runs `stat` first and skip anything whose `(size, mtime)` is unchanged, so the second run over an unchanged folder is a directory walk and nothing else.

Throughput therefore has to be stated per format, not as one number. The benchmark step below reports images per second by format and by stage, and it runs before any optimisation, so every optimisation has a measurement to justify it.

### No metadata, anywhere

Nothing reads EXIF, IPTC, XMP or an embedded colour profile. Most of the format set cannot carry them, they are stripped by half the tools that touch an image, and a fingerprint or a score that depends on them is one that changes for reasons that have nothing to do with the picture. Every value in the index comes from the file bytes or from the decoder output.

Three consequences worth naming, since each removes something an image tool would normally use:

- Times come from the filesystem. `mtime` is the only timestamp, which is what the incremental re-scan already keys on.
- Embedded thumbnails are not read, even though they are the single largest speed saving available for JPEG and TIFF. They are EXIF payload, they can be stale, and they can be cropped differently from the image.
- EXIF orientation is not applied. This costs nothing here: the dihedral canonicalisation already treats an image and its rotations as the same thing, so a file whose orientation flag was applied and one where it was not produce the same canonical hash regardless.

## The fingerprints

Different invariances cost very different amounts, and SIFT keypoint matching does not work at this size. SIFT produces a set of 128-dimensional descriptors per image, so comparing 30,000 images requires either an O(n^2) descriptor matching pass or a visual vocabulary index built first. Both take minutes to hours to answer a question that is usually "which of these are the same picture saved twice".

Three fingerprints per image instead, all cheap to compute and cheap to search:

**1. Content hash.** XXH3 over the file bytes, truncated to 32 bits. Finds byte-identical copies, which is most of what a folder of tens of thousands of images contains. Nothing here is adversarial, so there is no reason to pay for a cryptographic hash: this only has to be a fast digest with good distribution. The bytes are already read for the decode, so the hash is close to free.

32 bits is enough because the hash is a bucketing key and never a verdict. Two things make it so. Grouping is on `(size_bytes, content_hash)`, so an accidental collision also has to have an identical file size, and size is already stored. The group is then verified by comparing the files byte for byte before it is reported as a duplicate set, not at cleanup time, so a false pair never reaches the review UI.

Without both of those, 32 bits would be wrong: at 50,000 files the birthday bound puts roughly one accidental collision in every scan.

**2. Perceptual hash (63-bit), stored for all eight symmetries.** Reduce to 32x32 grayscale, 2D DCT, take the top-left 8x8 block excluding DC, threshold at the median, one bit per coefficient.

Scale and aspect invariance both come from resampling onto a fixed square: every image is mapped onto the same 32x32 grid whatever its size or shape, so a resize, a recompression or a format change produces the same hash or one a few bits away. It does not survive a change of framing, which is the cropping case, out of scope below.

Rotation passes through that resampling on its own. Resampling a rotated image onto the square gives the rotation of the resampled square, exactly, because the target is square and fixed. There is no orientation to correct for first. An earlier version of this plan argued the opposite, that a 4000x3000 image and its rotation are "squashed along different axes" and need turning to landscape before the resize; that is wrong, and a test at 4:1 measures it directly.

What does need care is which of the eight symmetries the hash is taken from, and the answer is: all of them. Each image stores the hash of its 32x32 grid and of that grid's four rotations and their mirrors, eight in total, and is indexed under all eight. Two images match if any of one image's eight is close to the other's first.

The alternative is a canonical orientation, one hash per image and an eighth of the index. Every way of choosing one is a hard decision on a continuous quantity: the numerically smallest of the eight hashes, or a comparison of low-frequency coefficients, or a dominant-orientation estimate. Resampling by a non-integral factor perturbs that quantity, so the decision flips, and a flipped decision does not move the hash by a few bits, it replaces it. Measured on the first implementation: a plain 2x downsample moved the canonical hash by 24 to 32 bits out of 63, which is chance. Storing all eight removes the decision, and a rotated pair then matches reliably rather than usually.

It costs one DCT, not eight. The symmetries of the image are a transpose and sign changes on the coefficients, so the eight hashes come from a single transform of the grid.

This covers the rotations that occur in practice: camera orientation, EXIF orientation flags applied or not applied, and 90 degree turns made on purpose. Rotation by an arbitrary angle is not covered. See the optional fingerprint below.

**3. Radial ring colour signature.** Concentric rings around the image centre, each summarised as a mean and a standard deviation in Oklab. Dividing the ring radii by the shorter side makes them scale invariant.

An earlier version also stored a quantised hue and chroma histogram. It was removed once it was measured: nothing queried it, and computing it doubled the Oklab conversions, which is the most expensive part of the fingerprint. Dropping it took the fingerprint stage from 809ms to 306ms over 304 JPEGs.

Rings are only rotation invariant if they stay inside the largest circle that fits in the image. Past that radius a ring is clipped by the top and bottom edges of a landscape image and by the left and right edges of its 90 degree rotation, so the two disagree. Rings are therefore limited to radius `min(w, h) / 2`, which confines them to the inscribed disc. The corners are not sampled, which is the cost, and inside that disc the signature is exactly rotation invariant at any angle, not just multiples of 90.

This is the confirmation step. A 64-bit pHash produces false positives on flat images, logos and screenshots, where many different pictures hash close together, and the colour signature rejects those.

It also disagrees with the pHash on one real case. The pHash is computed on grayscale, so a colourised copy and the grayscale original hash the same and are correctly matched. The colour signature then measures a large distance between them and throws the pair out. Which behaviour is right depends on what the folder is, so it is a setting: **match colour and grayscale versions as duplicates**. Off, the colour signature runs and the two are kept apart. On, it is skipped and they match. The signature is stored either way, so the setting is a bound parameter and switching it re-runs the match without re-indexing anything.

**Optional: log-polar Fourier magnitude.** For rotation by arbitrary angles, resample to log-polar coordinates around the centroid and take the magnitude spectrum. Rotation and scale become translations in that space, and the magnitude spectrum ignores translation. About 64 floats per image. This is a flag rather than the default because it costs measurable time and handles a case most folders do not contain. It gets built only if the other three are not enough on real data.

## Finding candidates without comparing everything to everything

Pairwise comparison of 30,000 images is 450 million comparisons. Avoiding that is the pigeonhole argument: split the hash into bands. If two hashes differ in fewer bits than there are bands, at least one band must be identical, so every near-duplicate pair collides in some band and looking up band equality finds all of them.

Two numbers decide whether that lookup is a lookup or a scan, and getting the first one wrong is what made the first version of this unusable.

**Band width has to be above `log2(n)`.** A band of `w` bits has `2^w` buckets, and `n` indexed hashes put `n / 2^w` in each. The first version used 8-bit bands: 256 buckets, and at 27,500 images with eight variants each that is around 860 hashes per bucket. The candidate join then produced roughly 190 million pairs and did not finish in nine minutes. At 16 bits the same corpus averages under four per bucket and the join finishes in 41 seconds. This is what sets the hash length: 16 bands of 16 bits needs 255 bits of hash, which is why the DCT block is 16x16 on a 64x64 grid rather than 8x8 on 32x32.

**The pigeonhole radius is a floor, not a ceiling.** Sixteen bands guarantee finding pairs within 15 bits. Setting the verification threshold to that number was measured to reject half of all rotated duplicates, because a rotation re-encodes on a shifted block grid and lands around 8 percent out, which is 20 bits. Those pairs are still generated as candidates: 20 bits spread over sixteen bands usually leaves several clean. So the thresholds come from the measured distances, not from the guarantee.

That is an indexed equality lookup, which is what a database does. The whole match runs in SQLite over the data already in it. No loading rows into memory, no rebuilding index structures SQLite already maintains, and the results persist between runs.

**Exact duplicates.** Grouped on size and hash together, so a 32-bit collision also needs a matching file size to survive.

```sql
SELECT size_bytes, bytes_hash, group_concat(id)
FROM files
GROUP BY size_bytes, bytes_hash HAVING count(*) > 1;
```

Each group is then verified by comparing its files byte for byte before it is reported, so nothing false reaches the review.

**Candidate pairs.** One self-join on `phash_bands`, using the `(band_index, band_value)` index. Because the bands are rows, the band count is a constant in the fingerprinting code and does not appear in the query at all.

One side is restricted to the variant the image was indexed under and the other contributes all eight, which is enough to find a rotated pair: if B is a symmetry of A then one of A's eight hashes is B's first. Leaving both sides open would produce every variant pairing and find nothing extra.

```sql
CREATE TEMP TABLE candidates AS
SELECT DISTINCT
       min(a.file_id, b.file_id) AS a,
       max(a.file_id, b.file_id) AS b
FROM phash_bands a
JOIN phash_bands b
  ON b.band_index = a.band_index
 AND b.band_value = a.band_value
 AND b.file_id <> a.file_id
WHERE a.variant = 0;
```

**Verification.** `hamming_any`, `ring_distance` and `aspect_ok` are scalar functions registered on the connection through `rusqlite`'s `create_scalar_function`, declared deterministic so SQLite can hoist and cache them. `hamming_any` takes one image's eight hashes and another's indexed one, and returns the closest.

```sql
CREATE TEMP TABLE matches AS
SELECT c.a, c.b FROM candidates c
JOIN indexed_images ia ON ia.id = c.a
JOIN indexed_images ib ON ib.id = c.b
WHERE hamming_any(ia.dct_hashes, ib.dct_hash) <= :max_bits
  AND (:ignore_colour OR ring_distance(ia.ring_stats, ib.ring_stats) <= :max_ring)
  AND aspect_ok(ia.width, ia.height, ib.width, ib.height);
```

`:ignore_colour` is the colour and grayscale setting. It is a bound parameter, so toggling it re-runs this one statement.

**Clustering.** Connected components as a recursive CTE, seeded only from the nodes that have edges, so the images with no match cost nothing. The minimum reachable id is the set identifier, which makes it stable across runs.

```sql
CREATE TEMP TABLE sets AS
WITH RECURSIVE
  edges(x, y) AS (SELECT a, b FROM matches UNION SELECT b, a FROM matches),
  reach(root, node) AS (
    SELECT x, x FROM edges
    UNION
    SELECT r.root, e.y FROM reach r JOIN edges e ON e.x = r.node
  )
SELECT node AS file_id, min(root) AS set_id FROM reach GROUP BY node;
```

`candidates`, `matches` and `sets` are all temp tables. They are derived from the index, not part of it, and they are rebuilt whenever the thresholds change.

A set can therefore contain an original, a resize of it, and a rotation of the resize, even where the original and the rotation never matched each other directly.

Thresholds and the colour setting are all bound parameters, so the controls in the UI rebind and re-run rather than recomputing anything. The right threshold depends on the folder, and no single number works for both a photo library and a folder of screenshots.

The threshold is a share of the hash, so it keeps its meaning if the hash length changes, and it is a control rather than three fixed points: a slider in the app, `--sensitivity` on the command line. The presets are positions on it, at 4, 10 and 16 percent.

It is a control because there is no right answer to set. Measured against 400 pictures at 1600px, each with an exact copy, a half-size copy and a 90 degree rotation planted, the exact and resized copies are all found everywhere on the scale and the rotations are what move:

| sensitivity | rotations found of 50 | wrong pairings |
| --- | --- | --- |
| 2% | 8 | 0 |
| 4% (strict) | 16 | 0 |
| 7% | 30 | 0 |
| 10% (balanced) | 41 | 0 |
| 13% | 47 | 0 |
| 16% (loose) | 49 | 0 |
| 20% (the cap) | 50 | 1 |

Rotation is the case that costs recall, because rotating a JPEG re-encodes it on a shifted block grid. The scale stops at 20 percent: that is where the first pairing of two unrelated pictures appeared, and unrelated pictures were measured above 25 percent apart.

## Which one to keep

Each image in a set gets a score, the highest wins, and the UI shows the reason next to it so that a wrong choice is visible.

In priority order:

1. **Resolution.** More pixels wins.
2. **Encoding.** A lossless format beats a lossy one at equal resolution. Beyond that, bytes per pixel, which works the same way for every format and does not pretend that one codec's quality number can be ordered against another's.
3. **Channels.** Colour beats grayscale, and alpha beats flattened. Read from the decoder output rather than from any header.
4. **Cropping.** If one image is a crop of another, prefer the one showing more, unless the cropped one is at a much higher resolution.
5. **Filename and path.** Reduce the score for the patterns copies have: a trailing `(1)`, `- Copy`, `_1`, a leading `copy of `, a `~` prefix, or a path containing a folder called `Copy`, `New folder` or `Downloads`. Prefer the shorter path and the earlier modification time, since the original is normally the one that existed first.

Ties are broken by earliest mtime, then by path, so the choice is deterministic across runs.

This is also a query. The score is an expression over columns already in the row, and the pick is a window function over the set:

```sql
SELECT s.set_id, i.id,
       row_number() OVER (
         PARTITION BY s.set_id
         ORDER BY keep_score(i.width, i.height, i.format, i.channels,
                             i.size_bytes, i.rel_path) DESC,
                  i.mtime_ns ASC, i.rel_path ASC
       ) = 1 AS auto_keep
FROM sets s JOIN indexed_images i ON i.id = s.file_id;
```

`keep_score` is registered the same way as `hamming`. Putting it in SQL means the automatic pick comes out of the same query as the sets, so the GUI receives a ready-made list and holds only the overrides the user makes on top of it.

## The database

One SQLite file, `imgdedupe.sqlite`, in the scanned root by default, with a flag to put it elsewhere. Everything is keyed on the path relative to the root, so the folder plus its index can be moved or copied without invalidating anything.

Tables are split by lifecycle: what invalidates a row is what decides which table it lives in. A filesystem change invalidates a file row. A fingerprint algorithm change invalidates only the fingerprints.

```
meta(key TEXT PRIMARY KEY, value TEXT)
    schema_version, root_path, recurse, last_scan

files(
    id               INTEGER PRIMARY KEY,
    rel_path         TEXT    NOT NULL UNIQUE,
    size_bytes       INTEGER NOT NULL,
    mtime_ns         INTEGER NOT NULL,
    bytes_hash       INTEGER NOT NULL,
    last_scanned_at  INTEGER NOT NULL
)

images(
    file_id            INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    width              INTEGER NOT NULL,
    height             INTEGER NOT NULL,
    format             TEXT    NOT NULL,
    channels           INTEGER NOT NULL
)

fingerprints(
    file_id             INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    fingerprint_version INTEGER NOT NULL,
    dct_hash            INTEGER NOT NULL,
    dct_hashes          BLOB    NOT NULL,
    ring_stats          BLOB    NOT NULL
)

phash_bands(
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    variant    INTEGER NOT NULL,
    band_index INTEGER NOT NULL,
    band_value INTEGER NOT NULL,
    PRIMARY KEY (file_id, variant, band_index)
) WITHOUT ROWID

CREATE INDEX files_bytes_hash ON files(size_bytes, bytes_hash);
CREATE INDEX bands_lookup     ON phash_bands(band_index, band_value);
```

There is no `quality` table any more. Every column in `images` is something the decoder reports about the picture, and nothing else is stored. The encoder-quality column is gone, because it could only ever have been per-codec and per-codec numbers cannot be ordered against each other. The sharpness measurement is gone, because it existed only to catch an upscaled copy, which `size_bytes` over pixel count already separates in nearly every real case. Bit depth is gone, because the display is 8-bit and it never changes which file a person would keep.

`channels` stays because it does: a grayscale copy of a colour image, or a flattened copy of one with alpha, is a visibly worse file to keep.

`bytes_hash` is on `files`, not `images`, because it hashes the file's bytes and not the picture.

`dct_hash` is the hash of the image as it sits, and `dct_hashes` is that one plus the seven for its rotations and mirrors, eight little-endian u64s in 64 bytes.

`phash_bands` is the pigeonhole decomposition of those eight, one row per band per variant rather than one column per band. Columns would be a repeating group, would need one index each, would turn the candidate query into an eight-way union, and would make changing the band count a schema migration. As rows it is one index, one join, and a constant.

This is the largest thing in the index by a wide margin, and the largest cost in indexing: eight variants times sixteen bands is 128 rows per image. Measured at 27,500 files the whole index came to 128 MB, so 4.7 KB per image, which puts 50,000 images at around 230 MB, and writing those rows is most of the indexing time. That is the price of rotation matching that works rather than nearly works, and it is why the thumbnails are not also in here.

Nothing here stores pixels. The review UI calls `decode_at_most` on the files currently on screen and keeps an in-memory LRU of the results, so the database holds only what the matching needs and stays small.

`files` holds images only. A file that is not one is not recorded, so it is re-sniffed on every scan, which costs a 16-byte read that the walk's `stat` has already paid for the seek on.

`fingerprint_version` sits on `fingerprints` rather than on the file, so changing the fingerprinting recomputes only that table and `phash_bands`, leaving `files` and `images` alone.

Every table here holds a fact about a file on disk. Duplicate sets, the automatic pick and the user's selections are none of those: they are derived per run and live in temp tables and in the application. Nothing about a review session is persisted.

A view reassembles the parts for the queries that want a whole image:

```sql
CREATE VIEW indexed_images AS
SELECT f.id, f.rel_path, f.size_bytes, f.mtime_ns, f.bytes_hash,
       i.width, i.height, i.format, i.channels,
       p.dct_hash, p.dct_hashes, p.ring_stats
FROM files f
JOIN images i       ON i.file_id = f.id
JOIN fingerprints p ON p.file_id = f.id;
```

### Incremental re-scan

1. Walk the tree, collecting `(rel_path, size, mtime_ns)`.
2. Load `files` joined to `fingerprints` in one query, giving `(rel_path, size_bytes, mtime_ns, fingerprint_version)`.
3. Rows in `files` whose path is gone: delete. `ON DELETE CASCADE` clears the derived tables.
4. Paths whose `(size, mtime)` matches and whose `fingerprint_version` is current: skip entirely, no file read.
5. Everything else, new or changed: fingerprint and upsert into `files`, `images`, `fingerprints` and `phash_bands` in one transaction.
6. A `--verify` flag additionally re-hashes files whose mtime matched, for the case where something rewrote a file without moving the clock.
7. A raised `fingerprint_version` re-runs step 5 for the stale rows only, leaving `files` and `images` untouched.

## The indexer is its own program

`imgindex` is a separate binary that does the indexing pass and nothing else. It is the only thing that writes to the database.

```
imgindex <folder> [--recurse] [--db <path>] [--verify] [--progress json|text] [--threads N]
```

That is the whole surface. Neither binary carries a benchmark mode, a corpus generator or any other switch that exists for development: measuring and fixture building happen outside the executables, in the test suite.

It is called `imgindex` because indexing images is all it does.

It walks, diffs against the existing index, decodes and fingerprints what changed, and exits. `--progress text` writes a human-readable line for a terminal. `--progress json` writes one JSON object per line to stdout, which is what the GUI reads:

```
{"event":"start","total":48213}
{"event":"progress","done":1200,"new":1150,"changed":40,"unchanged":10,"removed":3,"per_sec":1840}
{"event":"error","path":"a/b.png","message":"truncated file"}
{"event":"done","indexed":1193,"removed":3,"failed":7,"elapsed_ms":26100}
```

Exit code 0 for a completed pass, 1 for a usage or database error, 2 if it was cancelled. Files that fail to decode are reported as `error` events and counted in `failed`; they do not stop the pass.

The GUI spawns it, reads stdout, and drives the progress bar from the events. Three things follow from that split:

- A decoder that crashes or hangs on a malformed file takes down the indexer, not the GUI. The GUI sees a non-zero exit and reports it, and the index is intact up to the last committed batch.
- Cancelling is killing the process. Commits are batched inside transactions, so the database is consistent at whatever batch completed, and the next run picks up the rest because the diff is by `(size, mtime)`.
- The database is in WAL mode with the indexer as the single writer, so the GUI can hold a read-only connection open throughout and query the index while the pass runs.

It is also usable on its own: indexing a folder from a script or a scheduled job needs no GUI, and the GUI can open an index built by someone else's run.

## The GUI

`egui`, one window, three views.

**Scan.** Pick a folder, an "include subfolders" toggle, a "match colour and grayscale versions as duplicates" toggle, a slider for how different two pictures may be and still count as the same one with the three presets as buttons beside it, and a start button. Start spawns `imgindex` with the corresponding flags. During the pass: a progress bar with images per second and the counts of new, changed, unchanged and removed, all read from the JSON events, plus a list of files that failed to decode. Cancel kills the process.

**Review.** A list of duplicate sets, each one a row of images decoded on demand. Every set opens with the automatic choice already marked and a one-line reason next to it ("4000x3000, q92" against "1024x768, q78"). Clicking another image moves the keep mark. Double click opens a larger side-by-side comparison. A set can be marked resolved, skipped, or set to keep everything in it. Filters for set size, similarity and recoverable bytes, and a sort by recoverable bytes so the largest sets come first.

**Cleanup.** A summary of what will happen: how many files, how many bytes, and the full list, exportable as text. Deletion goes to the OS recycle bin by default. The alternatives are move to a chosen quarantine folder, or, behind an explicit confirmation, permanent deletion. Nothing is removed until this screen is confirmed, and the confirm button names the count.

The GUI binary also runs headless for scripting, with `report` (duplicate sets as JSON or CSV) and `clean --plan` / `clean --apply`. Indexing is not among them: that is `imgindex`, and both the GUI and a script call the same binary for it.

## Build order

1. **Scaffold.** Cargo workspace: `imgdedupe-core` (library, no GUI dependencies), `imgindex` (the indexer binary), `imgdedupe` (the GUI binary, which also runs headless). Core is where the tests are.
2. **Walk and database.** Directory walk, schema, incremental diff logic, with tests over a fixture tree covering added, removed, touched and unchanged files.
3. **Decode and fingerprint.** `decode_at_most` with one implementation per format, magic-byte detection, multi-frame rejection, orientation canonicalisation, and the three fingerprints. Tests check invariance directly: a fixture image and its resizes, recompressions, rotations and mirrors must produce matching canonical hashes, and different fixtures must not. The rotation tests use non-square fixtures, since square ones pass whether the canonicalisation is there or not.
4. **Benchmark.** Measured before any optimisation, so that if the numbers are not good enough the design changes here rather than later. Nothing about it ships: no benchmark mode and no corpus generator in either binary. The decoder question is settled by a test, which is why `zune-jpeg` is a dev-dependency.
5. **Matching.** The band self-joins, the registered scalar functions, and the recursive CTE for components, all in SQL. Tested against a fixture index with a known correct answer, and with `EXPLAIN QUERY PLAN` asserted to use the band indexes rather than scanning.
6. **Scoring.** `keep_score` and the window function that picks the keeper, with tests fixing the expected winner for each ordered pair of cases in the priority list above.
7. **Indexer binary.** `imgindex`, with both progress formats, the exit codes, and per-file error events. Tested by running the binary over a fixture tree and asserting on the JSON event stream and the exit code, including a killed run leaving a consistent database.
8. **GUI.** The three views over the same core library, spawning the indexer and parsing its events. Report and clean are also reachable headless from this binary.
9. **Cleanup.** Trash, quarantine, permanent, each behind the confirmation screen.
10. **Packaging.** `cargo build --release` puts both binaries in one directory, which is what the GUI expects: it looks for `imgindex` beside its own executable. Nothing is signed and there is no release pipeline. The bar is that the two run from wherever they are put.

## Decisions to reject before I start

- **HEIC, AVIF and JPEG XL are not supported.** This is what keeps the build free of C libraries.
- **WebP is decoded in full and shrunk after**, because the pure-Rust decoder has no scaled path. If the benchmark shows WebP dominating the runtime, the fix is to link libwebp for that one format.
- **Rotation invariance is 90 degrees only by default.** Arbitrary angles are a flag, built only if the data needs it. If the folders contain scanned or hand-rotated images at odd angles, it moves into the default path.
- **Rotation costs eight times the band rows.** Around 230 MB of index for 50,000 images, and most of the indexing time. The alternative is a canonical orientation and an eighth of that, which was measured and does not work: any canonicalisation is a hard decision that resampling flips, and a flip replaces the hash rather than moving it.
- **Rotated duplicates are the weakest case, and how weak is the user's to choose.** At the balanced position 41 of 50 planted rotations were found, against 50 of 50 for exact and resized copies; at the top of the scale all 50, with one wrong pairing. Rotating a JPEG re-encodes it on a shifted block grid, which moves the hash further than a resize does. There is no setting that is right for every folder, so it is a slider and not a constant.
- **The hash is unstable on flat pictures.** When most of the low-frequency coefficients sit near the median, small changes flip many bits at once. That is inherent to a median-thresholded hash and it is what the ring colour signature is there to catch.
- **No SIFT.** Keypoint matching does not work at this number of images without a visual vocabulary index, which is a much larger project. The three fingerprints handle resize, recompression, format change, rotation and mirror. They do not handle heavy cropping or substantial edits.
- **Deletion goes to the recycle bin.** Permanent deletion exists, is not the default, and needs a separate confirmation.
- **The database is written into the scanned folder.** That makes the folder and its index portable together, but it writes a file into the folder being scanned.
