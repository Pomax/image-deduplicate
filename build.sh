#!/bin/sh
set -e
cd "$(dirname "$0")"

suffix=""
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) os="windows"; suffix=".exe" ;;
    Darwin) os="macos" ;;
    *) os="linux" ;;
esac

out="dist/$os"
mkdir -p "$out"

# The name there goes first, or the move that follows would be onto the same file
# cargo hard linked it from, which is refused.
rm -f "$out/imgdedupe$suffix"

cargo build --release --workspace

# Cargo hard links the binary to a second name under deps. Dropping that name
# leaves the one in dist as the only one for it. Cargo links it again next time,
# which measured at under a second for a binary that did not change.
mv "target/release/imgdedupe$suffix" "$out/imgdedupe$suffix"
rm -f "target/release/deps/imgdedupe$suffix"

# Packed, which is about 60 percent off the linked size. Without upx on the path
# the build still produces a working binary, just a larger one. It is not used on
# macOS: a packed binary cannot be signed, and an unsigned one will not open.
if [ "$os" != "macos" ] && command -v upx >/dev/null 2>&1; then
    upx --best --lzma "$out/imgdedupe$suffix"
fi

# A copy at the root as well, so the one to run is where it has always been.
cp -f "$out/imgdedupe$suffix" "imgdedupe$suffix"

echo "imgdedupe is in $(pwd)/$out and $(pwd)"
