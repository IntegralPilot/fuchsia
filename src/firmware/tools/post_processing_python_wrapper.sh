#!/usr/bin/env bash
#
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Wraps a Python script for use via `fuchsia_post_processing_script()`.
#
# This serves two main purposes:
#  1. Calls the Python script with the correct build-time interpreter provided
#     by `fuchsia_post_processing_script()`
#  2. Translates paths so that the Python script can use all paths as-is without
#     having to apply any path manipulation.
#
# Usage:
#
# ```
# fuchsia_post_processing_script(
#     # Name can be anything.
#     name = "foo_post_processing",
#
#     # The first arg must be the Python script to call into, the remaining
#     # args are passed through.
#     #
#     # Assembly will additionally inject these args:
#     #  1. `-z <zbi>`: the path to the ZBI to post-process
#     #  2. `-o <output>`: the path to write the resulting image
#     post_processing_script_args = ["foo_script.py", "--foo", "bar"],
#
#     # Both this wrapper and the Python script must be provided as inputs, as
#     # well as any other resources the Python script needs.
#     post_processing_script_inputs = {
#         "//src/firmware/tools:post_processing_python_wrapper.sh": "post_processing_python_wrapper.sh",
#         "//path/to/script:foo_script.py": "foo_script.py",
#     },
#
#     # Point to this file as the entry point.
#     post_processing_script_path = "post_processing_python_wrapper.sh",
#
#     # Required for us to get the correct build-time Python interpreter.
#     use_python = True,
# )
# ````

set -e

# The first 3 args are always `-p <python_interpreter> <script>`.
if [[ "$1" != "-p" || $# -lt 2 ]]; then
  echo "Error: -p <python_interpreter> is required as the first argument" >&2
  echo "Make sure 'use_python = True' in fuchsia_post_processing_script()" >&2
  exit 1
fi
python_interpreter="$2"
shift 2

if [[ $# -lt 1 ]]; then
  echo "Error: Python script path is required as first arg" >&2
  exit 1
fi
python_script="$1"
shift 1

# Now accumulate the remaining passthrough args.
#
# One trick here is that Bazel and assembly provide paths differently:
#  * Bazel - relative to script dir
#  * assembly - relative to cwd
# In order to simplify life for the Python script, we convert the assembly -z/-o
# paths to be absolute, and then `cd` to the script dir. This makes it so that
# the Python script can use all paths directly without needing any special-case
# handling.
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    # Translate assembly paths to absolute.
    -z | -o)
      args+=("$1" "$(readlink -f "$2")")
      shift 2
      ;;
    *)
      args+=("$1")
      shift
      ;;
  esac
done

# Change directory so that all Bazel relative paths are valid as-is.
cd "$(dirname "${BASH_SOURCE[0]}")"

# Ensure that Python doesn't write bytecode into the board, which dirties
# the board and causes incremental build issues.
export PYTHONDONTWRITEBYTECODE="1"

# Run the script using the resolved interpreter, forwarding the other arguments.
exec "./${python_interpreter}" "${python_script}" "${args[@]}"
