#!/bin/bash
set -e
set -x

TARGETPLATFORM=$1
shift

./scripts/build-cross.sh "$TARGETPLATFORM"

docker buildx build --platform $TARGETPLATFORM . "$@"
