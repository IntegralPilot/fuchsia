# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#!/bin/bash
set -e

ffx trace start --background --buffering-mode streaming --buffer-size 16 --categories '#default,stress' --output "$FUCHSIA_DIR/thread_stress.fxt"
ffx component run --recreate core/trace_manager/workloads:thread_stress fuchsia-pkg://fuchsia.com/trace_thread_stress#meta/trace_thread_stress.cm
sleep 10
ffx trace stop
