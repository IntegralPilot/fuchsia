#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
FUZZER_URL="fuchsia-pkg://fuchsia.com/f2fs_reader-fuzzers#meta/f2fs_reader_fuzzer_component.cm"

echo "==== Enabling fuzzing ===="
ffx config set fuzzing true

echo "==== Building the fuzzer ===="
fx build //src/storage/f2fs_reader/fuzzers:f2fs_reader-fuzzers_package
echo "==== Resolving latest package ===="
ffx component resolve "$FUZZER_URL" || true
CORPUS_DIR=$(mktemp -d)
trap 'rm -rf "$CORPUS_DIR"' EXIT

echo "==== Creating empty test image for seed corpus ===="
touch "$CORPUS_DIR/empty_seed"

echo "==== Ensuring clean fuzzer state ===="
ffx fuzz stop "$FUZZER_URL" || true

OUT_DIR=/tmp/fuzzer_output
mkdir -p "${OUT_DIR}"

echo "==== Setting fuzzer max_input_size ==="
ffx fuzz set "$FUZZER_URL" max_input_size 1mb --output "${OUT_DIR}"

echo "==== Adding seed corpus ===="
ffx fuzz add "$FUZZER_URL" "$CORPUS_DIR" --seed --output "${OUT_DIR}"

echo "==== Configuring fuzzer for verbose output ===="
ffx fuzz set "$FUZZER_URL" debug true --output "${OUT_DIR}"
ffx fuzz set "$FUZZER_URL" purge_interval 3600s --output "${OUT_DIR}"
ffx fuzz set "$FUZZER_URL" asan_options "allocator_release_to_os_interval_ms=-1" --output "${OUT_DIR}"

DURATION=${1:-3600}
echo "==== Starting guided fuzzing (Duration: ${DURATION} seconds) ===="
echo "Note: To stop earlier, press Ctrl-C."
ffx fuzz run "$FUZZER_URL" --time "$DURATION"

echo "==== Fuzzing session completed ===="
