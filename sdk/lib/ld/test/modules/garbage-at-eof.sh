#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

if [ $# -ne 2 ]; then
  echo >&2 "Usage: $0 INPUT OUTPUT"
fi

readonly INPUT="$1"
readonly OUTPUT="$2"

set -e
trap 'rm -f "$OUTPUT"' HUP INT TERM

cp -f "$INPUT" "$OUTPUT"

echo "Nonzero garbage at end of file" >> "$OUTPUT"

exit 0
