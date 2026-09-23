#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"
SCRIPT_NAME="$(basename "${BASH_SOURCE[0]}")"

function die {
  echo >&2 "ERROR: $*"
  exit 1
}

case $OSTYPE in
    linux*)
      host_os=linux
      ;;
    darwin*)
      host_os=mac
      ;;
    *)
      die "Unknown operating system: $OSTYPE"
esac

case $MACHTYPE in
    x86_64*|amd64*)
      host_cpu=x64
      ;;
    aarch64*)
      host_cpu=arm64
      ;;
    *)
      die "Unknown CPU architecture: $MACHTYPE"
esac

FUCHSIA_SOURCE_DIR="$(cd "${SCRIPT_DIR}/../../.." && pwd 2>/dev/null)"
host_tag="${host_os}-${host_cpu}"

PREBUILT_PYTHON_PATH=${FUCHSIA_SOURCE_DIR}/prebuilt/third_party/python3/${host_tag}/bin/python3
export PYTHONDONTWRITEBYTECODE=1
exec "$PREBUILT_PYTHON_PATH" -S "${SCRIPT_DIR}/${SCRIPT_NAME%.sh}.py" "$@"
