#!/usr/bin/env bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Other
#### EXECUTABLE=${PREBUILT_3P_DIR}/python3/${HOST_PLATFORM}/bin/python3 ${FUCHSIA_DIR}/src/testing/perfcompare/perfcompare.py download_metrics
### Downloads metrics artifacts from CAS.
## Usage: fx download-metrics <build_id> <out_dir>
##
## Downloads performance test artifacts from Content Addressable Storage (CAS)
## into the specified output directory.
##
## Arguments:
##   build_id   The Buildbucket build ID to download metrics from.
##   out_dir    The directory to save the downloaded CAS contents into.
##              This directory will be created if it does not exist.
