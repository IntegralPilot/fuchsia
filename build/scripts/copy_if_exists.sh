#!/bin/sh

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Helper script to copy a source file to a destination if the source exists,
# or remove the destination file if the source does not exist. Optionally
# writes a depfile and touches a stamp file.

set -e

source="$1"
dest="$2"
depfile="$3"
stamp="$4"

if [ -f "$source" ]; then
  mkdir -p "$(dirname "$dest")"
  rm -f "$dest"
  cp -p "$source" "$dest"
  if [ -n "$depfile" ]; then
    echo "$stamp: $source" > "$depfile"
  fi
else
  rm -f "$dest"
  if [ -n "$depfile" ]; then
    echo "$stamp:" > "$depfile"
  fi
fi

if [ -n "$stamp" ]; then
  touch "$stamp"
fi
