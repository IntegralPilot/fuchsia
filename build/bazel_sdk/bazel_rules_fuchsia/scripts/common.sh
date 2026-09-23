#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

get_workspace_root() {
  local workspace_dir="${PWD}"
  while [[ "${workspace_dir}" != / ]]; do
    if [[ -e "${workspace_dir}/WORKSPACE" || -e "${workspace_dir}/WORKSPACE.bazel" ]]; then
      echo $workspace_dir
      return
    fi
    workspace_dir="$(dirname "${workspace_dir}")"
  done
  echo "Unable to find workspace root"
  exit 1
}

run_bazel() {
  local bazel="$(get_workspace_root)/tools/bazel"

  if [[ ! -x "${bazel}" ]]; then
    >&2 echo -n "[31;1m"
    >&2 echo "Bazel does not exist at ${bazel}. "
    >&2 echo -n "Please run the bootstrap script [scripts/bootstrap.sh] and try again:"
    >&2 echo "[0m"
    exit 1
  fi

  if [[ -z "$OUTPUT_BASE" ]]; then
    "${bazel}" "$@"
  else
    "${bazel}" --output_base="${OUTPUT_BASE}" "$@"
  fi
}

run_bazel_tool() {
  run_bazel run --ui_event_filters=-info,-stdout,-stderr --noshow_progress "$@"
}
