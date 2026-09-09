#!/bin/sh
set -e
# Decides whether this run builds and whether it releases, and under what tag.
# Writes build, release, and tag to $GITHUB_OUTPUT for the workflow to read.
cd "$(dirname "$0")/../.."

# The version from the [package] section of the Cargo.toml on stdin, ignoring
# the dependency versions further down the file.
read_version() {
    awk '
        /^\[(workspace\.)?package\]/ { in_package = 1; next }
        /^\[/          { in_package = 0 }
        in_package && /^[[:space:]]*version[[:space:]]*=/ {
            gsub(/.*=[[:space:]]*"|".*/, "")
            print
            exit
        }
    '
}

version=$(read_version < Cargo.toml)
if [ -z "$version" ]; then
    echo "Cargo.toml has no version in its [package] section" >&2
    exit 1
fi
echo "version in this commit: $version"

build=false
release=false
tag=""

previous=$(git show HEAD^:Cargo.toml 2>/dev/null | read_version)
echo "version in the previous commit: ${previous:-none}"
if [ "$version" = "$previous" ]; then
    echo "the version did not change, so there is nothing to build or release"
else
    build=true
    release=true
    tag="v$version"
fi

echo "build=$build" >> "$GITHUB_OUTPUT"
echo "release=$release" >> "$GITHUB_OUTPUT"
echo "tag=$tag" >> "$GITHUB_OUTPUT"
# The tag without its v, which is what the built archives are named after: a
# file sitting in somebody's downloads folder says which version it is.
echo "version=$version" >> "$GITHUB_OUTPUT"
