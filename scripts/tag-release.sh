#!/bin/sh
# This script sets things up to make a release,
# creating a tag based on the current commit.
TAG_NAME=${TAG_NAME:-$(git -c "core.abbrev=8" show -s "--format=%cd-%h" "--date=format:%Y.%m.%d")}
git tag $TAG_NAME
./scripts/apply-tag.sh
docker run -t -v "$(pwd)":/app/ "ghcr.io/orhun/git-cliff/git-cliff:${TAG:-latest}" -o addon/CHANGELOG.md -c scripts/cliff.toml
git add addon/config.yaml addon/CHANGELOG.md
git commit -m "Tag $TAG_NAME"
