#!/bin/sh
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e
OUTFILE="$1"

HASH_FILE="$2"
DATE_FILE="$3"

COMMIT_HASH="$(cat $HASH_FILE)"
COMMIT_DATE="$(cat $DATE_FILE)"

if [ -z "$COMMIT_HASH" ]; then
  echo >&2 "Commit hash file was invalid: $HASH_FILE"
  exit 1
fi

if [ -z "$COMMIT_DATE" ]; then
  echo >&2 "Commit date file was invalid: $DATE_FILE"
  exit 1
fi

VERSION_INFO="${COMMIT_HASH}-${COMMIT_DATE}"

# Update the existing file only if it's changed.
if [ ! -r "$OUTFILE" ] || [ "$(<"$OUTFILE")" != "$VERSION_INFO" ]; then
  echo "$VERSION_INFO" > "$OUTFILE"
fi
