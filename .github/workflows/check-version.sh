#!/bin/sh
set -e
# Decides whether this run builds and whether it releases, and under what tag.
# Writes build, release, and tag to $GITHUB_OUTPUT for the workflow to read.
cd "$(dirname "$0")/../.."

# The version from the [package] section of the Cargo.toml on stdin, ignoring
# the dependency versions further down the file.
read_version() {
    awk '
        /^\[package\]/ { in_package = 1; next }
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

if [ "$GITHUB_EVENT_NAME" = "pull_request" ]; then
    # Check that it still compiles, but release nothing.
    build=true
elif [ "$GITHUB_REF_TYPE" = "tag" ]; then
    build=true
    release=true
    tag="$GITHUB_REF_NAME"
else
    # A push to main, or a manual run. Only a changed version is a new release.
    previous=$(git show HEAD^:Cargo.toml 2>/dev/null | read_version)
    echo "version in the previous commit: ${previous:-none}"
    if [ "$version" = "$previous" ]; then
        echo "the version did not change, so there is nothing to release"
    else
        build=true
        release=true
        tag="v$version"
    fi
fi

echo "build=$build" >> "$GITHUB_OUTPUT"
echo "release=$release" >> "$GITHUB_OUTPUT"
echo "tag=$tag" >> "$GITHUB_OUTPUT"
