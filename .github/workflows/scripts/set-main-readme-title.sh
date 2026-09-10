#!/bin/sh
set -e

# usage: set-main-readme-title.sh <version>

VERSION=$1
if [ -z "$VERSION" ]; then
    echo "usage: set-main-readme-title.sh <version>" >&2
    exit 1
fi

git checkout main
git pull --ff-only origin main
printf '# imgdedupe v%s\n' "$VERSION" > README.next
tail -n +2 README.md >> README.next
mv README.next README.md
head -n 1 README.md

if git diff --quiet -- README.md; then
    echo "the title already says v$VERSION"
    exit 0
fi
git add README.md
git commit -m "v$VERSION"
git push origin main
