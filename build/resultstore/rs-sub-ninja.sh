#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# rs-sub-ninja.sh is a drop-in replacement for ninja that should be
# used in sub-builds when resultstore is enabled.
# This combines just rsproxy-wrap.sh and rsninja.sh.
# The reason for this is that each ninja invocation must be paired
# with its own rsproxy process; rsproxy was only designed for the
# single-invocation use case.
# If you need to inject other wrappers between rsproxy-wrap.sh
# and ninja, then compose your own command from these components.

set -euo pipefail

readonly SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

readonly fuchsia_rsproxy_wrap="$SCRIPT_DIR/fuchsia-rsproxy-wrap.sh"
readonly rsninja="$SCRIPT_DIR/rsninja.sh"

# Determine if we should bypass the rsproxy wrapper.
# Dry-runs, tools, and help commands do not produce build events or artifacts.
use_resultstore=1
for arg in "$@"
do
  case "$arg" in
    -n | --dry-run | -t | -t*)
      use_resultstore=0
      break
      ;;
    -h | --help | --version)
      use_resultstore=0
      break
      ;;
  esac
done

if [[ "$use_resultstore" == 0 ]]
then
  # Invoke rsninja.sh without the rsproxy wrapper.
  # rsninja.sh will detect that RSPROXY_FIFO is unset and fall back to plain ninja.
  exec "$rsninja" "$@"
else
  full_cmd=(
    "$fuchsia_rsproxy_wrap"
    --
    "$rsninja"
    "$@"
  )
  exec "${full_cmd[@]}"
fi
