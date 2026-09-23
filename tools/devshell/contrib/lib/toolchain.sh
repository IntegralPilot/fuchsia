#!/bin/bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"/../../lib/vars.sh || exit $?

function exec_toolchain_tool {
  if [[ $# -lt 1 ]]; then
    fx-command-help
    exit 1
  fi

  if [[ "$1" == -h || "$1" == "--help" ]]; then
    fx-print-command-help "$0"
    local msg="
The list of tools accessible via \`fx ${0##*/}\`"
    if [[ ${#TOOLCHAIN_PREFIXES[@]} -gt 1 ]]; then
      msg+=" (\`${TOOLCHAIN_PREFIXES[1]}\` prefix optional)"
    fi
    echo "$msg is:"

    local prebuilt_path tool
    for prebuilt_path in "${TOOLCHAIN_PATHS[@]}"; do
      for tool in "$prebuilt_path"/*; do
        if [[ -x "$tool" ]]; then
          echo "    ${tool#$prebuilt_path/}"
        fi
      done
    done
    exit 0
  fi

  local tool="$1"
  shift

  local prefix prebuilt_path
  for prefix in "${TOOLCHAIN_PREFIXES[@]}"; do
    for prebuilt_path in "${TOOLCHAIN_PATHS[@]}"; do
      if [[ -x "$prebuilt_path/$prefix$tool" ]]; then
        exec "$prebuilt_path/$tool" ${1+"$@"} || exit
      fi
    done
  done

  fx-error "Tool $tool not found in: ${TOOLCHAIN_PATHS[*]}"
}
