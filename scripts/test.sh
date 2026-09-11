#!/bin/sh
set -e
# The repository, which is where cargo has to be run from. This script lives in
# a directory of its own under it.
cd "$(dirname "$0")/.."

# The whole suite, the way CI runs it. Anything given on the command line is
# passed on to cargo, so one test can be run by name:
# scripts/test.sh the_name_of_the_test.
#
# The source is formatted before anything is compiled, so what the tests run
# against is what the formatter would leave behind. A formatter that will not
# run is source nobody can trust, so that stops the run here.
cargo fmt --all

exec cargo test --workspace "$@"
