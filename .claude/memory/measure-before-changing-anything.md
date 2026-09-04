---
name: measure-before-changing-anything
description: "in image-dedupe, take the measurement first; never reason from the code and ship a change"
metadata:
  node_type: memory
  type: feedback
---

Take one measurement before proposing or making any change, and do not call
anything fixed until it has been run against the real folder and the numbers say
so.

**Why:** reasoning from the code produced four wrong diagnoses in a row here. A
commit interval was set to half a second without ever asking what a commit cost
on that mount, which made indexing worse while it was being described as a fix. A
counter was pinned to a number known to be wrong at the time of writing it. The
instrumentation to answer every one of those questions already existed in the run
log and went unused until forced.

**How to apply:** find the number first, from `--log` or from one named test
against the real folder. State the number. Then change one thing. Related:
[[test-against-the-network-folder-only]], [[stay-inside-the-thing-that-was-asked]].
