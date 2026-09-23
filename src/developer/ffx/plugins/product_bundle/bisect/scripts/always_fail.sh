#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# This script always returns 1 (Fail).
# It is used by the product bundle bisection tool to simulate a failed validation step.
#
# The bisection tool passes the PB directory to the validation script:
#   --pb <path> : The directory containing the product bundle.
#
# Return Codes Expected by the Bisect Tool:
#   0 = Pass
#   1 = Fail
#   128+ = Abort (A fatal error occurred, stop the entire bisection process)
#

PB_PATH=""

# Parse arguments to find the paths passed by the bisect tool
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --pb) PB_PATH="$2"; shift 2 ;;
        *) shift ;;
    esac
done

echo "Received request to validate product bundle at: $PB_PATH"
echo "Returning Fail (1) immediately for testing."

# Exit with code 1 to indicate the validation failed
exit 1
