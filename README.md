# imgdedupe

A cross-platform desktop program that indexes a folder of images, finds duplicates in it, and removes the ones you don't keep. Written in Rust with egui. Builds to one executable, with no installer and no runtime dependencies.

## Supported image formats:

Supported basic formats:

- JPEG
- PNG
- GIF
- WebP
- TIFF
- HEIC

Supported RAW formats:

- Canon CR2/CR3
- Nikon NEF
- Sony ARW
- Panasonic RW2

Raw files are indexed from the JPEG preview the camera writes inside them: the sensor data itself needs the manufacturer's own demosaic, and the preview is the same picture. The size recorded is the raw's own, not the preview's. Files are identified by their contents; extensions are not consulted.

## The three steps

Each step has its own "page" in the UI.

**Scan.** Walks the folder (subfolders optional), decodes and fingerprints every file that is new or has changed, and writes the results to `imgdedupe.sqlite` in that folder. Files whose size and modification time match the index are skipped. Subfolders whose name begins with a dot or an at sign are not walked: `.git`, `.thumbnails`, `@eaDir` and the like belong to whatever made them. The folder the scan is pointed at is always read, whatever it is called.

"Only match within folders" makes a folder scanned with its subfolders into a search of each folder on its own: its own index of hashes, its own index of corners, holding what that folder holds. Two copies of a picture filed in two places are then two pictures and are never put together. It is off to begin with, and cannot be ticked without subfolders, since there is only one folder then. "Automatically rescan when opening this index" runs a pass over the files the moment the folder is opened, without waiting for the Scan button; it cannot be ticked without an index to rescan, and ticking the index box ticks it, since a folder worth keeping an index for is one worth bringing up to date on sight. Both are kept in the folder's index.

Opening a folder that has an index always reads that index into memory, whether or not it rescans: the pictures are then known and Find duplicates costs the comparing and nothing else. What the box decides is only whether the files themselves are looked at again.

What counts as a duplicate is set here: how far apart two pictures may be, whether colour is ignored, and which of the two ways of matching to use. "Match whole pictures" is the hash and the colour signature, which finds resizes, recompressions and rotations for almost nothing; "match partials" is the corners, which is what finds a crop and what most of a search's time goes on. Both are on to begin with, and both are kept in the folder's index, so a folder searched one way is searched that way again when it is opened.

Three bars report a run: read, indexed, and the duplicates scan, which is itself in three parts of roughly equal length, reading the index, drawing up the shortlist, and comparing what it produced.

Under them is every step a run goes through, in the order it goes through them, with the folder that was opened at the top. Green with the milliseconds since the run began — the press of Scan, or the reading of the index that opening a folder starts — means it happened, red means it has not happened yet, and an empty grey circle means there was nothing in it to do: a pass over a folder where nothing has changed reads no files and indexes none.

Pictures are shown the way up the file says they go: cameras record what the sensor read plus an orientation, and both the tiles and the preview apply it. The dimensions recorded are the picture's, so a portrait photograph reads 4040x6064 rather than the other way round.

**Review.** Shows each duplicate set as a row of thumbnails with one marked as the keeper. Click to preview, double click or space to mark and unmark, arrow keys to move, and a row of buttons along the bottom of each set: "keep all", "keep none", "ignore". With "allow multi-select" ticked, marking a picture adds to what the set keeps rather than replacing it; unticked, the mark moves. Either way a marked picture can be unmarked, which can leave a set keeping nothing. The choice is kept in the folder's index.

"Ignore" says the pictures in a set are not copies of one another. Every pair in the set is written to the `ignore` table in the folder's index, and a later search still finds the set and still shows it, so it can be seen and changed back; but a set every pair of which is ignored keeps nothing, drops nothing, and is stepped over by the cleanup entirely. A review holding nothing but ignored sets has nothing for a cleanup to do, so that step cannot be reached. Ignoring one pair of a larger set changes nothing: the rest of them are still copies.

An ignored set is drawn as one: everything above the buttons — the pictures and every line of writing under them — at a quarter of its opacity, no keeper's border and no ring round the one the preview is on, "keep all" and "keep none" out of reach because they mean nothing, and the third button reading "unignore". Pressing that takes the pairs out of the index and the set is a set again, drawn and worked with as it was, keeping the picture it was keeping when it was ignored. The cursor keys step over ignored sets: on to the next set that is one, and nowhere at all when there is none. Ignoring the set the preview is in leaves the preview where it is, so left and up go to the set before it and right and down to the set after it.

Cursor keys keep the picture they select on screen, scrolling the strip as far as it takes and no further. Clicking the preview fills the window with the picture at the window's own size; another click or the escape key puts it back.

Under the preview is what the file says about itself, in four sections: Image, Settings, Place, Description. It comes from Exif, IPTC and XMP, and from PNG text chunks, GIF comments and WebP chunks, whichever the file carries; raw files and HEIC are read the same way, including Canon's boxed directories. What is shown is what a photographer's panel shows and nothing else: shutter speed, aperture, ISO, focal length, exposure compensation, metering, flash, white balance, lens, dates, place, and the description fields an editor writes. Numbers that stand for words are shown as the words, dates read as "June 13, 2026, 4:18:34 am", and a field with no name worth showing is left out rather than printed as a tag number. The file is read on its own thread, so the window keeps drawing while a raw file arrives over the network.

**Clean up.** Removes everything not marked as kept, to the recycle bin, to a folder that mirrors the original structure, or by deleting. Failures are listed with their reason.

Dropping a folder onto the window does the first two steps: it opens the folder, scans it, and searches it.

## How duplicates are found

**Decode.** Each file is decoded to at most 128px on its long edge. JPEG uses [jpeg-decoder](https://github.com/image-rs/jpeg-decoder)'s DCT-scaled decode, which decodes at 1/2, 1/4 or 1/8 scale directly from the coefficients instead of decoding in full and downsampling. Raw files are searched for their embedded JPEG: the TIFF-based ones (CR2, NEF, ARW, RW2) by walking the [image file directories](https://web.archive.org/web/20240315014204/https://www.adobe.io/open/standards/TIFF.html) to the tags that point at it, CR3 by walking the [ISO base media](https://en.wikipedia.org/wiki/ISO_base_media_file_format) boxes. HEIC is [HEVC](https://en.wikipedia.org/wiki/High_Efficiency_Video_Coding) intra frames in that same container, decoded by [heic](https://github.com/imazen/heic); its thumbnail is used instead of the full picture when it is large enough to fingerprint.

**Perceptual hash.** The decoded image is converted to [Rec. 709](https://www.itu.int/rec/R-REC-BT.709) luma, resampled to a fixed 64x64 square (which is what makes the hash independent of scale and aspect ratio), and run through a 2D [DCT](https://en.wikipedia.org/wiki/Discrete_cosine_transform). The top-left 16x16 block of coefficients minus the DC term gives 255 bits, set by comparing each coefficient against the median of the block.

This is the DCT hash described in Zauner's [Implementation and Benchmarking of Perceptual Image Hash Functions](https://www.phash.org/docs/pubs/thesis_zauner.pdf) and in Krawetz's [Looks Like It](https://www.hackerfactor.com/blog/index.php?/archives/432-Looks-Like-It.html), with a 16x16 coefficient block rather than the usual 8x8, and a median threshold rather than a mean.

**Feature fingerprint.** The hash above describes the whole frame and is blind to a crop: cutting a picture down stretches a different region over the same square and every coefficient changes at once. So each picture is also described by what is in it. Corners are found with [FAST](https://www.edwardrosten.com/work/rosten_2006_machine.pdf) over an eight-level pyramid, each corner is given an orientation from its intensity centroid, and its neighbourhood is described as 256 brightness comparisons in a fixed rotated pattern, which is [BRIEF](https://www.cs.ubc.ca/~lowe/525/papers/calonder_eccv10.pdf) steered as in [ORB](https://ieeexplore.ieee.org/document/6126544). The strongest 320, spread over the frame by cell, are stored per picture: 11 KB.

Two pictures are compared by matching descriptions, keeping only matches clearly better than their runner-up and only where each corner is also the other's own best match, and then by geometry: a scale, a rotation and a shift take a picture onto a copy of it or onto the part a crop kept, so pairs of matches propose such an arrangement by [RANSAC](https://en.wikipedia.org/wiki/Random_sample_consensus) and the rest vote. Sixteen corners agreeing on one arrangement is a duplicate.

Both halves of that matter. Without the second rule, line art and repeated texture give a dozen corners of one picture the same best corner in the other, and a dozen matches that all end in the same place will agree on an arrangement that shrinks one picture onto that place: measured on unrelated drawings, exactly sixteen corners "agreeing" on an arrangement that put all of them on two places in the other picture, which was enough to report them as the same. Measured on photographs: unrelated pairs never got past ten, a picture and a 60% crop of it agreed on thirty-seven, frames of the same scene on sixty to two hundred. Nothing here is learned from the folder; a file's fingerprint depends only on that file.

**Rotation and mirroring.** All eight symmetries of the square are hashed and stored, rather than deriving a canonical orientation. Resampling a rotated image onto the square produces the rotation of the resampled square, so the eight hashes are exact, and a rotated copy matches without a per-image orientation decision that resizing could flip.

**Colour signature.** 12 concentric rings inside the inscribed circle, each contributing mean L, mean a, mean b and the standard deviation of L in [Oklab](https://bottosson.github.io/posts/oklab/). Rings are rotation invariant by construction. This is what separates a colourised copy from its grayscale original, and the "match colour with grayscale" checkbox skips the check.

**Candidate selection.** Comparing every pair is quadratic and does not scale. Each hash is cut into 16-bit bands and only images sharing a band are compared, which is the banding scheme used for [locality-sensitive hashing](https://en.wikipedia.org/wiki/Locality-sensitive_hashing) in chapter 3 of [Mining of Massive Datasets](http://www.mmds.org/). Two hashes differing by fewer bits than there are bands must agree on at least one band, so everything within that radius is guaranteed to be found; in practice the shortlist reaches considerably further, because scattered differing bits tend to leave some band untouched. Bands are computed while loading the index, not stored.

**Candidate selection for the corners.** The corners are indexed the same way the hashes are, since a description is also a string of bits where near means alike. Each picture files its strongest 128 corners under a sample of each description's bits, sixteen samples over, so two descriptions a few bits apart share a bucket in at least one of the sixteen. A picture then asks with its strongest 48 corners and reads a few hundred filed corners instead of the whole folder, and three corners within 24 of their 256 bits is enough to look properly.

The number of buckets follows the folder, so a folder ten times the size is ten times the buckets and the same reading per picture. A bucket holding more than 32 times the average is skipped: a description thousands of pictures share cannot say which picture this is, and measured on 9490 photographs those buckets were nineteen twentieths of the work and none of the answer. Measured on that folder: the corner pass takes 8 seconds and the whole search 21, against the 45 million pairs and roughly an hour and a quarter that comparing every pair would have cost. On subsets of it where comparing every pair was still affordable, it found what comparing every pair found.

**Verification.** Each candidate pair is compared properly: [Hamming distance](https://en.wikipedia.org/wiki/Hamming_distance) against all eight variants of the other image, taking the closest, then the ring distance unless colour is being ignored. Surviving pairs are merged into sets with a [disjoint-set structure](https://en.wikipedia.org/wiki/Disjoint-set_data_structure), with byte-identical images folded together first so a hundred copies of one file compare as one.

**Threshold.** The sensitivity slider is a percentage of the hash length, so the numbers keep their meaning if the hash length changes. Measured: resizes, recompressions and rotations of one picture move the hash by up to about 8%, unrelated images sit above 25%. Presets are close (5%), balanced (15%, the default), wide (30%) and yolo (50%).

**Keeper selection.** Scored per image: log2 of the pixel count dominates, then a budget smaller than one resolution doubling covering lossless format, bytes per pixel, colour, alpha, and filename or folder markers that indicate a copy (`- Copy`, `(1)`, `/downloads/`).

## Concurrency

Decoding and fingerprinting run on all cores through rayon; results go down a channel to a single writer thread that commits in transactions of 5000 records. Comparison is split across cores in batches. Scan, search and cleanup each run off the UI thread and report progress through channels the frame loop drains.

Thumbnails use two worker pools: up to 24 threads serving what is currently on screen, and 4 reading ahead, with the read-ahead pool held while anything on screen is still missing. Decoded images become GPU textures on the frame that draws them.

## The index

`imgdedupe.sqlite` in the scanned folder: tables for files (path, size, mtime), images (dimensions, format, channels), fingerprints (packed hashes, ring stats, corners), ignore (pairs of pictures said not to be copies of each other), and a meta table holding the schema version, last scan time, cleanup destination, recursion flag and the review's own settings. About 13 KB per picture, most of it the corners. An index written by an earlier build gains the columns and tables it lacks when opened, and its files are read again when the fingerprint version has moved on. A pass works on the index in memory and writes it out whole when it ends, so a folder on another machine costs one transfer rather than a round trip per statement; a `-wal` or `-shm` left beside an index by an older build is cleared when the index is closed.

The "Save an index database for this folder" checkbox controls whether that file is kept. Unticking it deletes the index immediately; a cleanup on an unticked folder deletes it when finished. The checkbox state comes from whether the folder contains an index.

## Building

Requires [Rust](https://rustup.rs).

- `scripts\build.bat` on Windows, `scripts/build.sh` elsewhere. Either can be run from anywhere: they work on the repository they are in, not on the directory you are standing in.

The executable is left in the repository root and is not committed. Built ones come from the releases. Release builds use fat LTO, one codegen unit, stripped symbols, `panic = "abort"`, and `opt-level = "z"` for the toolkit crates.

On Windows and Linux the result is compressed with UPX if installed. UPX-style binary compression is disallowed by MacOS and so not used on that platform.

## Releases

`.github/workflows/release.yml` builds all three platforms on GitHub's own runners, each with the build script above, and uses no third-party actions. The three go out together as `imgdedupe-windows.zip`, `imgdedupe-macos.zip` and `imgdedupe-linux.zip`, each holding the executable under its own name, `imgdedupe.exe` on Windows and `imgdedupe` everywhere else: a `v*` tag makes a release of its own, and a push to main replaces the one called `latest`, so the newest build of all three is always downloadable. A pull request builds all three and releases nothing.

## Tests

Run `scripts\test.bat` (on Windows) or `scripts/test.sh` (Everywhere else). Arguments are passed to Cargo, so a single test can be run by name. [tests.md](./tests.md) documents every test.

You run a special logging-enabled build by using `scripts\build.bat --test` or `scripts/build.sh --test` as argument to the build script, which yields a binary that lets you run it with the `--log` flag. This will cause it to log a ton of information to a log file in order to assist in debugging.

## Questions and comments

If you found a bug, fixed one, or want it to do something it doesn't, file an issue over on https://github.com/Pomax/image-deduplicate

For less serious engagement, https://mastodon.social/@TheRealPomax
