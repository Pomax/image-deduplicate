---
name: test-against-the-network-folder-only
description: "in image-dedupe, verify a fix against the folder in the app's settings file, never against a temp directory of generated files"
metadata:
  node_type: memory
  type: feedback
---

A fix is verified against the folder in the application's own settings file, which
is on a network mount. Calling a fix good on the strength of a temporary directory
of generated files is forbidden.

**Why:** every fault in this program is a latency fault. On a fast disk the walk,
the index read, the search and the cancel flag all complete in milliseconds, so a
run there passes against broken code and proves nothing. Two separate fixes were
verified against eight generated PNGs in a temp directory and shipped as working
while the real folder still took thirty seconds and would not close.

**How to apply:** read the folder from `crate::settings::Settings::load().folder`
or take it from the environment. If there is no folder set, fail loudly rather
than falling back to a temp directory. Related:
[[measure-before-changing-anything]], [[one-test-at-a-time]].
