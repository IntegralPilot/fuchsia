#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Script that, when symlinked from //tools/<TOOLNAME>, will attempt to
# find TOOLNAME in the //tools/<host_architecture/ directory in a Bazel SDK
# located in the "fuchsia_sdk" Bazel workspace.
#
# If the workspace has not been downloaded yet, for example in a new checkout,
# this script will attempt to bootstrap it first.
#
# How to use this script:
#
#    Assuming that sdk-integration is used in your repo as a git submodule in
#    //third_party/sdk-integration, create a symbolic link from //tools/TOOLNAME
#    to this script. For example, from the root of your repo:
#
#        ln -s ../third_party/sdk-integration/bazel_rules_fuchsia/tools/run_sdk_tool.sh tools/ffx
#
#     then you can execute 'tools/ffx' normally.
set -e

main() {
  root_dir=$( cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd )

  # Bootstrapping bazel must be done from inside a Bazel workspace,
  # or it will fail silently. https://fxbug.dev/42062594
  pushd "${root_dir}" > /dev/null

  tool_name=$( basename "${BASH_SOURCE[0]}" )
  if [[ ! -x "${root_dir}/tools/bazel" ]]; then
    if [[ -x "${root_dir}/scripts/bootstrap.sh" ]]; then
      echo >&2 "INFO: Cannot find Bazel, attempting to fetch it..."
      "${root_dir}/scripts/bootstrap.sh"
    fi
    if [[ ! -x "${root_dir}/tools/bazel" ]]; then
      echo >&2 "ERROR: cannot fetch bazel. Ensure scripts/bootstrap.sh is able to fetch tools/bazel."
      return 1
    fi
  fi

  # Redirect stderr to hide the output of `bazel info`. Flags --ui_event_filters
  # and --noshow_progress don't hide "Starting local Bazel server and connecting to it..."
  # which could be misleading/confusing to someone running a tool
  bazel_dir="$("${root_dir}/tools/bazel" info output_base 2>/dev/null)"
  # Attempt to notify the user that a download will take place
  if [[ ! -d "${bazel_dir}/external/fuchsia_sdk" ]]; then
    echo >&2 "INFO: Cannot find the Fuchsia SDK toolchain, attempting to fetch it..."
    bazel_opts="--show_progress"
  else
    # Note: we might still have to do a download if the SDK version changes and the
    # user will not see progress.
    bazel_opts="--noshow_progress"
  fi

  # Run the tool and forward any command line arguments to the tool.
  exec "${root_dir}"/tools/bazel run \
    --run_under="cd $PWD && " \
    --ui_event_filters=-info,-stderr "${bazel_opts}" \
    "@fuchsia_sdk//:${tool_name}" -- "$@"

  popd > /dev/null # "${root_dir}"
}
main "$@"

