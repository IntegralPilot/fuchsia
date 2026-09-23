#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A cp wrapper that only adds `-R` if SRC is a dir, so it works on MacOS.
#
# Usage: ./cp.sh SRC DEST [OPTIONS...]

set -euo pipefail

if [[ -d $1 ]]; then
  cp -R $@
else
  cp $@
fi
