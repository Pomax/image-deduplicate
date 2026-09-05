#!/bin/sh
set -e
cd "$(dirname "$0")"

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

case "$(uname -s)" in
    Darwin) os="macos" ;;
    # There is no one Linux to build for, so the distribution names the output:
    # ID from /etc/os-release, which every distribution ships.
    *) os="$(. /etc/os-release 2>/dev/null && echo "$ID")"; os="${os:-linux}" ;;
esac

out="dist/$os"
mkdir -p "$out"

# The name there goes first, or the move that follows would be onto the same file
# cargo hard linked it from, which is refused.
rm -f "$out/imgdedupe"

cargo build --release --workspace $features

# Cargo hard links the binary to a second name under deps. Dropping that name
# leaves the one in dist as the only one for it. Cargo links it again next time,
# which measured at under a second for a binary that did not change.
mv "target/release/imgdedupe" "$out/imgdedupe"
rm -f "target/release/deps/imgdedupe"

# Packed, which is about 60 percent off the linked size. Without upx on the path
# the build still produces a working binary, just a larger one. It is not used on
# macOS: a packed binary cannot be signed, and an unsigned one will not open.
if [ "$os" != "macos" ] && command -v upx >/dev/null 2>&1; then
    upx --best --lzma "$out/imgdedupe"
fi

# A copy at the root as well, so the one to run is where it has always been.
cp -f "$out/imgdedupe" "imgdedupe"

echo "imgdedupe$note is in $(pwd)/$out and $(pwd)"
