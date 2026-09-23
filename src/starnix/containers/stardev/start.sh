#!/bin/sh
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

ffx component run /core/starnix_runner/playground:stardev fuchsia-pkg://fuchsia.com/stardev#meta/stardev_container.cm
