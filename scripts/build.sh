#!/bin/sh
set -e
# The repository, which is where cargo has to be run from and where the built
# executable belongs. This script lives in a directory of its own under it.
cd "$(dirname "$0")/.."

# With --test the binary understands --log and writes a run log. Without it none
# of that code is compiled in: the flag, the file, and every line the program
# would have written are all behind the feature.
features=""
note=""
if [ $# -gt 0 ]; then
    if [ "$1" = "--test" ]; then
        features="--features imgdedupe/logging"
        note=" --log build"
    else
        echo "the only argument is --test" >&2
        exit 1
    fi
fi

cargo build --release --workspace $features

# The one at the root goes, or the move that follows would be onto the same file
# cargo hard linked it from, which is refused. A run that is going does not
# notice: on these systems a program holds the file it was started from, not the
# name, and the name is all that is being taken away.
rm -f imgdedupe

# Cargo hard links the binary to a second name under deps. Dropping that name
# leaves the one at the root as the only one for it. Cargo links it again next
# time, which measured at under a second for a binary that did not change.
mv "target/release/imgdedupe" imgdedupe
rm -f "target/release/deps/imgdedupe"

# Packed, which is about 60 percent off the linked size. Without upx on the path
# the build still produces a working binary, just a larger one. It is not used on
# macOS: a packed binary cannot be signed, and an unsigned one will not open.
if [ "$(uname -s)" != "Darwin" ] && command -v upx >/dev/null 2>&1; then
    upx --best --lzma imgdedupe
fi

echo "imgdedupe$note is in $(pwd)"
