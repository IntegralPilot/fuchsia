#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

if [ -z "$1" ]; then
  echo "Usage: $0 <fuchsia-version>"
  echo "Example: $0 17.20240101.0.1"
  exit 1
fi

VERSION=$1
TARGET_DIR="$(dirname $0)/$VERSION"

echo "Looking up minimal.x64 version $VERSION..."
URL=$(ffx product lookup minimal.x64 $VERSION)

echo "Downloading $URL to $TARGET_DIR..."
ffx product download --force "$URL" "$TARGET_DIR"

echo "Stripping large artifacts and unneeded directories..."
mv "$TARGET_DIR/product_bundle.json" "${TARGET_DIR}.json"
rm -rf "$TARGET_DIR"
mkdir -p "$TARGET_DIR"
mv "${TARGET_DIR}.json" "$TARGET_DIR/product_bundle.json"

echo "Done."
