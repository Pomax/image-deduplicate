#!/bin/sh
set -e

# usage: put-on-releases-branch.sh <tag> <version>

TAG=$1
VERSION=$2
if [ -z "$TAG" ] || [ -z "$VERSION" ]; then
    echo "usage: put-on-releases-branch.sh <tag> <version>" >&2
    exit 1
fi

if git ls-remote --exit-code --heads origin releases >/dev/null 2>&1; then
    git fetch origin releases
    git checkout releases
else
    git checkout --orphan releases
    git rm -rf --quiet .
fi

if [ -d "$VERSION" ]; then
    echo "$VERSION is already on the releases branch" >&2
    exit 1
fi
mkdir "$VERSION"
mv built/*.zip "$VERSION"/
ls -l "$VERSION"

# Update the README, which lists all releases
if [ ! -f README.md ]; then
    printf '# imgdedupe releases\n' > README.md
fi
made_on=$(git log -1 --format=%cs "$GITHUB_SHA")
sum() { sha256sum "$VERSION/imgdedupe-$VERSION-$1.zip" | cut -d' ' -f1; }
{
    head -n 1 README.md
    printf '\n## %s (%s)\n\n' "$VERSION" "$made_on"
    printf -- '- [Windows](./%s/imgdedupe-%s-windows.zip) (sha256 %s)\n' \
      "$VERSION" "$VERSION" "$(sum windows)"
    printf -- '- [macOS](./%s/imgdedupe-%s-macos.zip) (sha256 %s)\n' \
      "$VERSION" "$VERSION" "$(sum macos)"
    printf -- '- [Linux](./%s/imgdedupe-%s-linux.zip) (sha256 %s)\n' \
      "$VERSION" "$VERSION" "$(sum linux)"
    printf -- '- [Source code](../../tree/%s)\n' "$TAG"
    tail -n +2 README.md
} > README.next
mv README.next README.md
cat README.md

# Update the release log (with a placeholder text)
if [ ! -f RELEASE_LOG.md ]; then
    printf '# Release log\n' > RELEASE_LOG.md
fi
{
    head -n 1 RELEASE_LOG.md
    printf '\n## %s (%s)\n\nPlease update the notes for this version\n' \
      "$VERSION" "$made_on"
    tail -n +2 RELEASE_LOG.md
} > RELEASE_LOG.next
mv RELEASE_LOG.next RELEASE_LOG.md
head -n 5 RELEASE_LOG.md

git add "$VERSION" README.md RELEASE_LOG.md
git commit -m "$VERSION"
git push origin releases
