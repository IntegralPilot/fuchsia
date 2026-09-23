#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A host test wrapper used to launch a host test using the Fuchsia test runners.
# Should be consistent with the output of //build/testing/create_test.sh for non Python
# host tests.

# See comments in //build/bazel/rules/host_tests/host_test.bzl for details.
set -e
cd -- "$(dirname "${BASH_SOURCE[0]}")/{{runtime_dir_location}}"
export RUNFILES_DIR="${PWD}/{{test_name}}.runfiles"
export RUNFILES_MANIFEST_FILE="${RUNFILES_DIR}/MANIFEST"
{{ld_library_path_export}}
echo "starting tool {{test_name}}"
exec {{env_vars}} "./{{test_name}}" {{test_args}} "$@"
