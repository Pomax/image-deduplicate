#!/bin/sh
set -e
# The repository, which is where cargo has to be run from. This script lives in
# a directory of its own under it.
cd "$(dirname "$0")/.."

# Runs the whole suite. Anything given on the command line is passed on to
# cargo, so one test can be run by name: scripts/test.sh the_name_of_the_test
cargo test --workspace "$@"
