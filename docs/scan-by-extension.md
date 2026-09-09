# A scan looks at the name before it reads the file

A pass reads every file in the folder in full and then looks at the first 4096
bytes to decide whether it is a picture. `dirlist::read_whole` reads the whole
file; `index_one` calls `format::detect` on the head of what came back. A folder
with a 200 MB video in it reads all 200 MB to find out it is not a JPEG, and over
a network that is 200 MB across the wire.

The name says it first. A file is a candidate if its extension is one of the ones
these formats use, and the magic number then confirms that it is what the name
claims. That is the order: name to choose, bytes to verify.

It also removes the reason the walk knows about the index and the files SQLite
keeps beside it. None of them end in a picture's extension, so none of them are
candidates, and the walk stops naming them.

## Tasks

- [x] 1. Add the extensions each format uses
- [x] 2. Keep only files whose extension is one of them
- [x] 3. Delete the list of index files the walk skips
- [x] 4. Named tests for each of the above
- [x] 5. Update `docs/tests.md`
- [x] 6. Run the named tests, then build

## 1. Add the extensions each format uses

In `crates/imgdedupe-core/src/format.rs`, beside the `Format` enum and
`Format::as_str`:

- `pub fn extensions(self) -> &'static [&'static str]`, the extensions that
  format is written under, lower case and without the dot: `jpg`, `jpeg`, `jpe`
  and `jfif` for `Jpeg`; `tif` and `tiff` for `Tiff`; `heic` and `heif` for
  `Heic`; one each for the rest.
- `pub fn from_extension(name: &str) -> Option<Format>`, matching an extension
  against those lists, without regard to case.

The formats are the eleven in the enum: JPEG, PNG, GIF, WebP, TIFF, HEIC, and the
five raw families.

## 2. Keep only files whose extension is one of them

In `crates/imgdedupe-core/src/scan.rs`, in the walk, where each file is added to
the list of candidates: a file whose extension is not one of those is not a
candidate, and nothing reads it.

The magic number stays where it is. `format::detect` is still what decides the
format an indexed file is treated as, so a JPEG named `.png` is indexed as a
JPEG, and a file named `.jpg` that is not one is still refused.

## 3. Delete the list of index files the walk skips

In `crates/imgdedupe-core/src/scan.rs`, delete the `sidecars` list and the check
against it. In `crates/imgdedupe-core/src/db.rs`, delete
`files_of_the_index` if nothing else calls it.

The index is `imgdedupe.sqlite` and the files SQLite keeps beside it end in
`-journal`, `-wal` and `-shm`. None of those is a picture's extension, so the
walk passes over them without being told about them.

## 4. Named tests for each of the above

- Every format's extensions map back to that format, and an unknown extension
  maps to nothing.
- An extension in capitals is the same extension.
- A folder holding a file with no picture extension, the index itself, comes
  back from the walk without it, and the file is not read.
- A file named `.jpg` that is not a JPEG is still refused, so the magic number is
  still what decides.
- A JPEG named `.png` is indexed as a JPEG.

## 5. Update `docs/tests.md`

An entry for every test above, and corrections to the entries for the tests that
described the walk skipping the index by name.

## 6. Run the named tests, then build

The suite, then `scripts\build.bat`.
