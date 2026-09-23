#!/bin/bash
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Runs test scripts using the bash_test_framework.
#
set -e
SCRIPT_SRC_DIR="$(cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd)"
TEST_DIR="$PWD"

function find_tree_root {
  local parent="$1"
  if [[ ! -d "$parent" ]]; then
    return 1
  fi
  while [[ ! -f "${parent}/.fx-root" ]]; do
    if [[ "$parent" == "/" ]]; then
      return 1
    fi
    parent="$(dirname "${parent}")"
  done
  echo "$parent"
}

launch_script() {
  local test_script_name=""
  test_script_name="$1"
  shift
  local test_script_path="${SCRIPT_SRC_DIR}/${test_script_name}"
  local bt_deps_root="${TEST_DIR}";

  # Check the local directory as the root of the test framework first,
  # then fall back to locating //.fx-root.
  local test_framework_path="${bt_deps_root}/tools/devshell/tests/lib/bash_test_framework.sh"
  if [[ ! -e "${test_framework_path}" ]]; then
    echo "Could not find $test_framework_path"
    if ! bt_deps_root="$(find_tree_root "${SCRIPT_SRC_DIR}")"; then
      echo >&2 "ERROR: Cannot find the Platform Source Tree in a parent of the test directory: ${SCRIPT_SRC_DIR}"
      exit 1
    fi
    test_framework_path="${bt_deps_root}/tools/devshell/tests/lib/bash_test_framework.sh"
  fi

  if [[ ! -f "${test_script_path}" ]]; then
    echo >&2 "Test script '${test_script_path}' not found. Aborting."
    return 1
  fi
  # propagate certain bash flags if present
  local shell_flags=()
  if [[ $- == *x* ]]; then
    shell_flags+=( -x )
  fi

  # Start a clean environment, load the bash_test_framework.sh,
  # then start the test script.
  # No quotes around EOF so variables are expanded when heredoc is processed.
  local -r launch_script="$(cat << EOF
  #set -x
export BT_DEPS_ROOT="${bt_deps_root}"
cd "\$BT_DEPS_ROOT"
source "${test_framework_path}" || exit \$?
source "${test_script_path}" || exit \$?
EOF
)"

echo "Launching test script $test_script_path"

  /usr/bin/env -i \
      USER="${USER}" \
      HOME="${HOME}" \
      bash "${shell_flags[@]}" \
      -c "${launch_script}" "${test_script_path}" "$@"
}

launch_script "$@"
