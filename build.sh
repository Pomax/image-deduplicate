#!/bin/sh
set -e
cd "$(dirname "$0")"

suffix=""
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) suffix=".exe" ;;
esac

# The name here goes first, or the move that follows would be onto the same file
# cargo hard linked it from, which is refused.
rm -f "imgdedupe$suffix"

cargo build --release --workspace

# Cargo hard links the binary to a second name under deps. Dropping that name
# leaves the one here as the only one for it. Cargo links it again next time,
# which measured at under a second for a binary that did not change.
mv "target/release/imgdedupe$suffix" "imgdedupe$suffix"
rm -f "target/release/deps/imgdedupe$suffix"

# Packed, which is about 60 percent off the linked size. Without upx on the path
# the build still produces a working binary, just a larger one.
if command -v upx >/dev/null 2>&1; then
    upx --best --lzma "imgdedupe$suffix"
fi

echo "imgdedupe is in $(pwd)"
