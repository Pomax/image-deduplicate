---
name: one-test-at-a-time
description: "in image-dedupe, run the single test for the thing that is broken, never the suite"
metadata:
  node_type: memory
  type: feedback
---

Run one test, by name, for the thing that is broken. Not the suite, not a module,
not "everything that could be affected".

**Why:** a suite run buries the one number that matters in a hundred results that
were never in question, and most of those tests are against local storage where
the fault does not exist. It also takes the machine for minutes at a time.

**How to apply:** `cargo test --bin imgdedupe <exact_test_name>` or
`cargo test -p imgdedupe-core --lib <exact_test_name>`. Related:
[[test-against-the-network-folder-only]].
