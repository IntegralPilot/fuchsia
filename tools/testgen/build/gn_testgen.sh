#!/bin/sh

# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Helper script for testing the testgen tool.
# The first argument is testgen's output directory. We'll remove this before
# running testgen since GN creates it eagerly and testgen refuses to use an
# existing output directory. The second argument is the testgen binary, and
# the remaining arguments are for testgen.

set -e

output_dir="$1"
shift

testgen="$1"
shift

if [ -d "$output_dir" ]; then
  rm -rf $output_dir
fi

exec "$testgen" "$@"