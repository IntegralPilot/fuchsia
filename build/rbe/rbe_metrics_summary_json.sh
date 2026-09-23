#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Format RBE metrics summary to JSON for infra recipes to consume.

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"

# Get path to python interpreter.
source "$script_dir"/common-setup.sh

exec "$python" -S "$script_dir"/build_summary.py --format=json "$@"
