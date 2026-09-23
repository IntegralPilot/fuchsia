#!/usr/bin/env bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script compiles the .proto files and copies the compiled versions
# from the build dir back to the source tree. It should be run whenever
# the .proto files are modified.

set -o errexit
set -o pipefail

# cd to fuchsia checkout root.
cd "$( dirname "${BASH_SOURCE[0]}" )/../.."

scripts/fx set core.x64 --with //tools/metadata:proto_lib
scripts/fx ninja -C out/default tools/metadata:proto_lib
cp out/default/gen/go-proto-gen/src/tools/metadata/proto/*.pb.go \
  tools/metadata/proto/
