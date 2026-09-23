#!/bin/sh
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

BUILD_DIR=$(fx get-build-dir)
$BUILD_DIR/host_x64/blobfs /tmp/blob@1236992 create
$BUILD_DIR/host_x64/fvm golden-fvm.blk create --slice 32768 --blob /tmp/blob --with-empty-data
