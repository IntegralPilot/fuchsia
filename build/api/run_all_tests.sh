#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

_script_dir="${BASH_SOURCE[0]%/*}"; if [[ "${_script_dir}" == "${BASH_SOURCE[0]}" ]]; then _script_dir=.; fi

# Assume this script is under //build/api/
readonly FUCHSIA_DIR="${_script_dir}/../.."

# shellcheck source=/dev/null
source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh" || exit 1

exec "$PREBUILT_PYTHON3" -S "${FUCHSIA_DIR}"/build/testing/python_build_time_tests.py --source-dir "${_script_dir}"
