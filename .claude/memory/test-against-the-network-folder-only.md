---
name: test-against-the-network-folder-only
description: "in image-dedupe, verify against the folder in the app's settings file, never against a local temp directory"
metadata:
  node_type: memory
  type: feedback
---

Every check runs against the folder in the application's own settings file, which
is on a network mount. Running anything against a temporary directory of generated
files on the local disk is forbidden.

**Why:** every fault in this program is a latency fault. On local storage the
walk, the index read, the search and the cancel flag all complete in
milliseconds, so a test there passes against broken code and proves nothing. Two
separate fixes were verified against eight generated PNGs in a temp directory and
shipped as working while the real folder still took thirty seconds and would not
close.

**How to apply:** read the folder from `crate::settings::Settings::load().folder`
or take it from the environment. If there is no folder set, fail loudly rather
than falling back to a temp directory. Related:
[[measure-before-changing-anything]], [[one-test-at-a-time]].
