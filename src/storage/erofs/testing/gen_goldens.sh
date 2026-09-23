#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
set -euo pipefail

# Get the directory of this script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"

cd "$SCRIPT_DIR"

cleanup() {
  # Clean up local xattrs set during image generation. git doesn't track xattrs, so we want to make
  # sure everything is in a reproducible and clean state.
  setfattr -x user.flavor data/simple/file1 2>/dev/null || true
  setfattr -x user.security data/simple/file1 2>/dev/null || true
  setfattr -x user.shared data/simple/file1 2>/dev/null || true
  setfattr -x user.shared data/simple/photosynthesis 2>/dev/null || true

  # Clean up temporary large directory and symlink
  rm -rf data/simple/large_dir
  rm -f data/simple/symlink_to_file1
}
trap cleanup EXIT

# Recreate the golden files even if they exists
rm -f data/simple.erofs
rm -f data/simple_512.erofs
rm -f data/simple_lz4.erofs
rm -f data/simple_lz4_legacy.erofs

mkdir -p data/simple/large_dir
for i in $(seq 1 50); do
  echo "file $i" > "data/simple/large_dir/file_number_$i"
done

ln -s file1 data/simple/symlink_to_file1

# Set inline xattrs on file1
setfattr -n user.flavor -v "vanilla" data/simple/file1
setfattr -n user.security -v "high" data/simple/file1

# Set identical xattrs to multiple files to force mkfs.erofs to use shared xattrs
setfattr -n user.shared -v "same_value" data/simple/file1
setfattr -n user.shared -v "same_value" data/simple/photosynthesis

mkfs.erofs --file-contexts=data/file_contexts -b 4096 data/simple.erofs data/simple
mkfs.erofs --file-contexts=data/file_contexts -b 512 data/simple_512.erofs data/simple
mkfs.erofs --file-contexts=data/file_contexts -b 4096 -z lz4 data/simple_lz4.erofs data/simple
mkfs.erofs --file-contexts=data/file_contexts -b 4096 -z lz4 -E legacy-compress data/simple_lz4_legacy.erofs data/simple

echo "All golden EROFS images generated successfully."
