#!/bin/sh
# Copyright 2021 The Fuchsia Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT

ABI="$1"
IMPL="$2"
OUTPUT="$3"

rm -f "$OUTPUT"

if diff -U0 "$ABI" "$IMPL"; then
  touch "$OUTPUT"
  exit 0
fi

echo >&2 "
*** ABI mismatch ***

This suggests that the tooling that creates zircon.ifs has regressed.
Please file a bug and notify the current maintainers in
//zircon/tools/zither/OWNERS.

*** ABI mismatch ***
"

exit 1
