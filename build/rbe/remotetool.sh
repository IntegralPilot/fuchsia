#!/bin/bash
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# See https://github.com/bazelbuild/remote-apis-sdks
# `remotetool` can be used to inspect past remote actions, re-execute them,
# fetch artifacts, etc.

set -uo pipefail

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"  # dirname
readonly script_basename="${script##*/}"  # basename
readonly stem="${script_basename%.sh}"

source "$script_dir"/common-setup.sh
# 'python' is defined

exec "$python" -S "$script_dir"/"$stem".py "$@"
