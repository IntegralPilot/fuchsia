#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
COMMON_SCRIPT="${SCRIPT_DIR}/../../lib/zero_function/run_zero_test_common.sh"
TEST_TARGET=\
"//src/tests/end_to_end/usb/stress/zero_function:zero_function_stress_test"

exec "${COMMON_SCRIPT}" \
    --caller "$0" \
    --test-name "zero_function_stress_test" \
    --test-target "${TEST_TARGET}" \
    --is-stress \
    "$@"
