#!/bin/bash
#
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# This script runs either the C++ or Rust state_recorder example,
# assuming a suitable emulator is running.
#
# Usage: ./run_example.sh <cpp|rust>

if [ "$#" -ne 1 ] || ( [ "$1" != "cpp" ] && [ "$1" != "rust" ] ); then
  echo "Usage: $0 <cpp|rust>"
  exit 1
fi

LANG=$1  # "rust" or "cpp"
EXAMPLE="state_recorder_${LANG}_example"
if ffx component list 2>/dev/null | grep -q ${EXAMPLE}$; then
  ffx component destroy /core/ffx-laboratory:${EXAMPLE}
fi
ffx trace start --categories kernel:meta,power_example --duration 15 &

# Give `ffx trace start` a chance to start running before launching the
# component. Empirically, 2 works better than 1 on a VM.
sleep 2

ffx component run \
  /core/ffx-laboratory:${EXAMPLE} \
  "fuchsia-pkg://fuchsia.com/${EXAMPLE}#meta/${EXAMPLE}.cm"
wait %1
