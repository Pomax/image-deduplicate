#!/bin/sh
set -e

cd "$(dirname "$0")/../../.."

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
echo "version=$version" >> "$GITHUB_OUTPUT"
