#!/bin/sh
# This script sets things up to make a release,
# creating a tag based on the current commit.
TAG_NAME=${TAG_NAME:-$(git -c "core.abbrev=8" show -s "--format=%cd-%h" "--date=format:%Y.%m.%d")}
git tag $TAG_NAME
./apply-tag.sh
git add addon/config.yaml
git commit -m "Tag $TAG_NAME"
