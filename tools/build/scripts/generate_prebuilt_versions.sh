#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script is expected to be invoked as a Jiri hook from
# integration/fuchsia/stem.

# Its purpose is to generate a prebuilt_versions.json manifest
# that describes the versions of prebuilts included in the checkout.
# This is needed to know the versions of emulator packages to use
# to launch emulators and run tests on. The location of the output
# file will be stored in the prebuilt_versions_location build API module.

_SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

fatal () {
  echo >&2 "FATAL: $*"
  exit 1
}

# Assume this script lives under //tools/build/scripts/
FUCHSIA_DIR="$(cd "${_SCRIPT_DIR}/../../.." && pwd -P 2>/dev/null)"
if [[ ! -f "${FUCHSIA_DIR}/.jiri_manifest" ]]; then
  fatal "Cannot locate proper FUCHSIA_DIR, got: ${FUCHSIA_DIR}"
fi
JIRI_SNAPSHOT="${FUCHSIA_DIR}/.jiri_root/update_history/latest"
OUTPUT_FILE="${FUCHSIA_DIR}/.jiri_root/prebuilt_versions.json"

source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh" || exit 1

$PREBUILT_PYTHON3 "${_SCRIPT_DIR}/generate_prebuilt_versions.py" --jiri-snapshot "${JIRI_SNAPSHOT}" --output "${OUTPUT_FILE}"
