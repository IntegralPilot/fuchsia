#!/usr/bin/env bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Other
#### EXECUTABLE=${PREBUILT_3P_DIR}/python3/${HOST_PLATFORM}/bin/python3 ${FUCHSIA_DIR}/src/testing/perfcompare/perfcompare.py extract_metrics
### Extracts metrics from Fuchsia performance results.
## Usage: fx extract-metrics <suite_prefix> [data_dir] [--out-file <path>]
##
## Traverses a directory tree to find `.fuchsiaperf.json` files and
## extracts metrics under the provided test suite prefix.
## The results are aggregated and sorted deterministically.
##
## Arguments:
##   suite_prefix The test suite prefix by which to filter metrics (e.g. fuchsia.microbenchmarks).
##   data_dir     The directory to process. Defaults to the current directory.
##
## Options:
##   --out-file   A path to save the resulting CSV. By default, output will
##                be printed to stdout.
