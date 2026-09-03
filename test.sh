#!/bin/sh
set -e
cd "$(dirname "$0")"

# Runs the whole suite. Anything given on the command line is passed on to
# cargo, so one test can be run by name: ./test.sh the_name_of_the_test
cargo test --workspace "$@"
