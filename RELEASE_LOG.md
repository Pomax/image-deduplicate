# Release log

## 0.6.0 (2026-09-10)

- Fixed a MacOS issue around loading indexes for network locations, where it would seemingly just lock up.

## 0.5.0 (2026-09-09)

- Fixed the preview metadata scrollbar refusing to actually scroll to the bottom

## 0.4.0 (2026-09-08)

This version fundamentally changes how review works, as a session-based, "you make the decisions but there are helpers to let you perform broad actions" review system

- Reviews are now sessions, stored in your index (if you use one of course) so you can simply close the app mid-review and resume at a later date
- Folder rescanning was drastically improved. IF you have a stored session but there are folder differences, you will be given the choice to resume your old session, or update your index and start a new session
- Images can be marked/unmarked via space/double click, "only pick this image" is set to shift+space/shift+doubleclick
- Review has three convenience buttons to either unmark everything, mark everything, or automatically mark each set with a single image that the code thinks is the most probable image worth keeping.
- You can now cancel scans with the "esc" key
- Working with network folders is way faster now.
- The app tries to use as much ram as your machine will let it, because that's what you bought that ram for. The whole point of RAM is to speed up otherwise-disk-based operations

## 0.3.0 (2026-09-06)

The window title now includes the version number so you know what you're running. Tiny change, but the whole point is better release accountability so it's its own release.

## 0.2.0 (2026-09-06)

- added support for HEIC, TIFF, and RAW formats (Canon CR2 and CR3, Nikon NEF, Sony ARW and
  Panasonic RW2)
- Added file information to the preview panel
- Added matching on partials
- Improved scan and index times
- Sets can be marked as "ignore" so the app does not treat those images as duplicates of each other even though they are.
- You can mark more than one image as "keep" in duplicate sets now.
- There's now a program icon.

## 0.1.0 (2026-09-03)

Initial version.

The app has three parts: a scan, a review, and a clean up. The scan parses folder content and (optionally) stores that in an index file (a sqlite database) so that you don't have to reparse an entire folder or tree with thousands of files every time you open the app.

Images are "fingerprinted" on several properties for fast comparison, and there is a slider to say how narrow or loose you want the matching to be when you run a duplicates search.

Duplicates are presented as sets where you can decide which image in the set to keep.

Once all sets have a review, clean up can be told to either delete all unmarked files (either hard delete or recoverably via the recycle bin/trash), or move them to a dedicated folder.
