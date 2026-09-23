#!/bin/sh
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
set -eu

# Check if the test_deps is already mounted, returning early if so.
if grep -qs ' /mnt/test_deps ' /proc/mounts; then
  echo "test_deps already mounted"
  exit 0
fi

# Create the mount points. Since the ROMFS is read-only, we'll also prepare an OverlayFS to allow
# the tests write access to the subdirectories of the mount point.
mkdir -p /mnt/test_deps /data_rw /data_work /data

# Iterate through the available block devices, trying to find the one
# tagged with the "test_deps" serial attribute.
DEV=""
for d in /sys/class/block/vd*; do
  if [ -f "$d/serial" ] && [ "$(cat "$d/serial" 2>/dev/null)" = "test_deps" ]; then
    DEV="/dev/$(basename "$d")"
    break
  fi
done

if [ -z "$DEV" ]; then
  echo "Error: test_deps block device not found. Available block devices:" >&2
  ls -la /sys/class/block/ >&2
  for d in /sys/class/block/vd*; do
    echo "$d serial: $(cat $d/serial 2>/dev/null || echo 'none')" >&2
  done
  exit 1
fi

# Mount the test_deps and prepare the overlay.
mount -t romfs -o ro "$DEV" /mnt/test_deps
mount -t overlay overlay -o lowerdir=/mnt/test_deps,upperdir=/data_rw,workdir=/data_work /data
echo "Successfully mounted overlay at /data"
