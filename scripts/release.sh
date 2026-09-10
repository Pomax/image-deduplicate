#!/bin/sh
set -e
# The repository, which is where cargo and git have to be run from. This script
# lives in a directory of its own under it.
cd "$(dirname "$0")/.."

case "$1" in
    major|minor|patch) ;;
    *)
        echo "usage: scripts/release.sh major|minor|patch" >&2
        exit 1
        ;;
esac

# Releases are cut from main and nowhere else.
if [ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then
    echo "not on main"
    exit 1
fi

# The version the whole workspace shares. It is the one in the
# `[workspace.package]` section, not any of the dependency versions further
# down the file.
current=$(awk '
    /^\[/ { here = ($0 == "[workspace.package]") }
    here && /^version[[:space:]]*=/ {
        gsub(/[^0-9.]/, "")
        print
        exit
    }
' Cargo.toml)
if [ -z "$current" ]; then
    echo "Cargo.toml has no version in its [workspace.package] section" >&2
    exit 1
fi

major=${current%%.*}
rest=${current#*.}
minor=${rest%%.*}
patch=${rest#*.}

case "$1" in
    major) major=$((major + 1)); minor=0; patch=0 ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    patch) patch=$((patch + 1)) ;;
esac
next="$major.$minor.$patch"
echo "$current to $next"

awk -v version="$next" '
    /^\[/ { here = ($0 == "[workspace.package]") }
    here && !changed && /^version[[:space:]]*=/ {
        print "version = \"" version "\""
        changed = 1
        next
    }
    { print }
' Cargo.toml > Cargo.toml.next
mv Cargo.toml.next Cargo.toml

# The lock file carries the version of every crate in the workspace, so it has
# to say the same thing the manifest now says. Only the workspace's own entries
# are touched: the dependencies stay on what they were locked to.
cargo update --workspace

git add Cargo.toml Cargo.lock
git commit -m "bump to v$next"
