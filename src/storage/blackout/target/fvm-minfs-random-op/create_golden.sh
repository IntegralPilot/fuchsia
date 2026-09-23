#!/bin/sh
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

BUILD_DIR=$(fx get-build-dir)
$BUILD_DIR/host_x64/fvm golden-fvm.blk create --slice 32768 --length $((60 * 1024 * 1024))
# Truncate the image so it just contains the metadata.
truncate golden-fvm.blk --size 180224