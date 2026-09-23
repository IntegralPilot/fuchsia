#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -euE -o pipefail

function swap() {
  local -r file1=$1
  local -r file2=$2
  python3 -c 'import sys; import ctypes; syscall=ctypes.CDLL(None).syscall; syscall.restype=ctypes.c_int; syscall.argtypes=(ctypes.c_long, ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint); sys.exit(syscall(316, -100, sys.argv[1].encode("utf-8"), -100, sys.argv[2].encode("utf-8"), 2))' "$file1" "$file2" || (
    echo "failed to exchange snapshots: $?"
    return 1
  )
}

MOUNTPOINT=""
MULTIFUCHSIA_WORKSPACE=""

while (( $# )); do
  case "$1" in
    --mount)
      shift
      MOUNTPOINT="$1"
      ;;
    --workspace)
      shift
      MULTIFUCHSIA_WORKSPACE="$1"
      ;;
    *)
      echo "Unrecognized option: $1"
      exit 1
      ;;
  esac
  shift
done

readonly CLEAN_DIR="$MULTIFUCHSIA_WORKSPACE/clean"
export FUCHSIA_DIR="$CLEAN_DIR"
export PATH="$CLEAN_DIR/.jiri_root/bin:$PATH"

function bind_mount() {
  local -r source="$1"

  if mount --bind "$source" "$MOUNTPOINT" ; then
    # all good
    true
  else
    if findmnt --raw --noheadings --output 'SOURCE' --mountpoint "$MOUNTPOINT" >/dev/null; then
      echo "Working around mounted but broken $MOUNTPOINT" >&2
      local -r mountpoint_parent="$(dirname $MOUNTPOINT)"
      mount -t tmpfs fake_src "$mountpoint_parent"
      mkdir -p "$MOUNTPOINT"
      mount --bind "$source" "$MOUNTPOINT"
    else
      echo "Mount failed but nothing seems to be mounted on $MOUNTPOINT" >&2
      return 1
    fi
  fi
}

if [ ! -z "${MOUNTPOINT}" ]; then
  bind_mount "$CLEAN_DIR"
fi

(
  if [ -z "${MOUNTPOINT}" ]; then
    cd "${CLEAN_DIR}"
  else
    cd "${MOUNTPOINT}"
  fi

  ./.jiri_root/bin/jiri update -gc

  ./scripts/fx build
)

if [ -d "${MULTIFUCHSIA_WORKSPACE}/snapshots/build.new" ]; then
  echo "Deleting stale snapshot build.new" >&2
  btrfs property set "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new ro false
  btrfs subvolume delete -c "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new
fi
btrfs subvolume snapshot -r "${CLEAN_DIR}" "${MULTIFUCHSIA_WORKSPACE}/snapshots/build.new"
if [ -d "${MULTIFUCHSIA_WORKSPACE}/snapshots/build.success" ]; then
  swap "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.success
  btrfs property set "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new ro false
  btrfs subvolume delete -c "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new |& grep -v 'WARNING: cannot read default subvolume id: Operation not permitted'
else
  mv "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.new "${MULTIFUCHSIA_WORKSPACE}"/snapshots/build.success
fi
echo "snapshots/build.success updated" >&2

(
  if [ -z "${MOUNTPOINT}" ]; then
    cd "${CLEAN_DIR}"
  else
    cd "${MOUNTPOINT}"
  fi
)
