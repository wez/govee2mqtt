#!/bin/bash
set -e
set -x

echo "Rust build disabled; Python v2 is the primary implementation."
exit 0

TARGETPLATFORM=$1
shift

./scripts/build-cross.sh "$TARGETPLATFORM"

docker buildx build --platform $TARGETPLATFORM . "$@"
