#!/bin/sh
set -e
# The repository, which is where cargo has to be run from. This script lives in
# a directory of its own under it.
cd "$(dirname "$0")/.."

# Without arguments this is the suite, the way CI runs it: no extra features,
# and nothing that reads anybody's real folder of photographs.
#
# With `--all` it is that plus the checks in `local/`, which are compiled by the
# `local` feature and run against the folder the application is set to. They take
# minutes and they write to a real index, so they are asked for.
#
# Anything else given on the command line is passed on to cargo, so one test can
# be run by name: scripts/test.sh the_name_of_the_test.
if [ "$1" = "--all" ]; then
    shift
    if [ ! -d local ]; then
        echo "there is no local/ here, so there is nothing --all adds" >&2
        exit 1
    fi
    # The suite, without them: `local::` is the module they are all in, and
    # everything else runs the way it always does, in parallel.
    cargo test --workspace --features imgdedupe/local -- --skip local::
    # Then those, one at a time. Not because they are optional: the feature is
    # what asks for them, and asking for it is asking for them to run. It is that
    # there is one real index and they all open it, so run together they read
    # each other's half-written work and three of the seven fail on it.
    exec cargo test --workspace --features imgdedupe/local local:: \
        -- --nocapture --test-threads=1
fi

exec cargo test --workspace "$@"
