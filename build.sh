#!/bin/sh
set -e
cd "$(dirname "$0")"

suffix=""
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) suffix=".exe" ;;
esac

# The name here goes first, or the move that follows would be onto the same file
# cargo hard linked it from, which is refused.
for name in imgindex imgdedupe; do
    rm -f "$name$suffix"
done

cargo build --release --workspace

# Cargo hard links each binary to a second name under deps. Dropping that name
# leaves the one here as the only one for it. Cargo links it again next time,
# which measured at under a second for a binary that did not change.
for name in imgindex imgdedupe; do
    mv "target/release/$name$suffix" "$name$suffix"
    rm -f "target/release/deps/$name$suffix"
done

echo "imgindex and imgdedupe are in $(pwd)"
