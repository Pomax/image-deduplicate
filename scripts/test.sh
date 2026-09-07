#!/bin/sh
set -e
# The repository, which is where cargo has to be run from. This script lives in
# a directory of its own under it.
cd "$(dirname "$0")/.."

# Anything given on the command line is passed on to cargo, so one test can be
# run by name: scripts/test.sh the_name_of_the_test. A named run is what was
# asked for and nothing else is added to it.
if [ $# -gt 0 ]; then
    exec cargo test --workspace "$@"
fi

# `local/` holds the checks that only mean something against a real folder of
# photographs. It is not in the repository, so a checkout without it runs the
# suite exactly as CI does, and a machine that has it runs those as well.
if [ ! -d local ]; then
    cargo test --workspace
    exit
fi

echo "local/ is here, so the checks against a real folder are included"
cargo test --workspace --features imgdedupe/local

# They are all marked to be asked for by name, because they take minutes and
# work on a real index. `local::` is the module they are all in.
#
# One at a time: there is one index and they all take it up, so run together
# they read each other's half-written work and three of the seven fail on it.
cargo test --workspace --features imgdedupe/local local:: \
    -- --ignored --nocapture --test-threads=1
