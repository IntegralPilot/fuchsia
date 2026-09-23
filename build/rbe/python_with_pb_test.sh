#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Common dispatcher for standalone python binaries and tests,
# that need an adjusted PYTHONPATH to point to compiled python pb code.
# To use this script, symlink a .sh to this script, using the python script's
# basename.

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"
readonly script_basename="${script##*/}"

# 'stem' is any executable python binary or test
stem="${script_basename%.sh}"

source "$script_dir"/common-setup.sh
# 'python' is defined

script_dir_abs="$(normalize_path "$script_dir")"
project_root="$default_project_root"

generated_src=api/log/log_pb2.py
test -f "$script_dir"/proto/"$generated_src" || {
  cat <<EOF
Generated source $script_dir/proto/$generated_src not found.
Run $script_dir/proto/refresh.sh first.
EOF
  exit 1
}

readonly PROTOBUF_WHEEL="prebuilt/third_party/protobuf-py3"

env \
  PYTHONPATH="$script_dir_abs":"$script_dir_abs"/proto:"$project_root/$PROTOBUF_WHEEL" \
  "$python" \
  -S \
  "$script_dir"/"$stem".py \
  "$@"
