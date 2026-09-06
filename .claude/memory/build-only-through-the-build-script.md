---
name: build-only-through-the-build-script
description: "in image-dedupe, build and move binaries by running build.bat, never cargo commands typed out by hand"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 9482fd90-5f9e-4853-b7eb-ed2cebd3b253
  modified: 2026-09-01T02:47:14.378Z
---

In `image-dedupe`, building is `scripts\build.bat` (or `scripts/build.sh` off
Windows). Never run `cargo build --release --workspace` and the two `mv` calls by
hand.

**Why:** those commands were being retyped every time. A build that lives only in
a chat history is not a build anyone else can run, and the steps drift.

**How to apply:** run `scripts\build.bat`, from wherever you are: it works on the
repository it is in rather than on the directory you are standing in. If the
build needs to
change, change the script. Related: [[never-run-a-test-you-do-not-save]].
