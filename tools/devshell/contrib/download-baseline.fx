#!/usr/bin/env bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Other
#### EXECUTABLE=${PREBUILT_3P_DIR}/python3/${HOST_PLATFORM}/bin/python3 ${FUCHSIA_DIR}/src/testing/perfcompare/perfcompare.py download_baseline
### Finds the most recent successful sample build for a given builder and downloads its metrics.
## Usage: fx download-baseline [-n <count>] <builder_name> <out_dir>
##
## Fetches a list of sample builds for a given builder and iterates through
## the last <n> builds to find the most recent success.
## It then downloads the performance artifacts for that build into <out_dir>.
##
## Arguments:
##   builder_name  The name of the builder to filter by (e.g., terminal.x64-release).
##   out_dir       The directory to save the downloaded CAS contents into.
##
## Options:
##   -n <count>    The number of builds to check (default: 5).
