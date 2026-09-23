#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
This script extracts realistic 128 KiB chunks from a Fuchsia build to use as
test data for storage benchmarks (measuring cold page fault performance).

It scans the `all_blobs.json` file in the build directory to locate built blobs,
reads them, and compresses 128 KiB chunks of their data using LZ4 and ZSTD.
The compression is performed in 32 KiB sub-chunks to match Fuchsia's delivery
blob chunked compression format.

It finds chunks that closest match target compression ratios (40%, 55%, 70%)
and saves them to the sibling `test_data` directory.

NOTE: This script must be run against a public release build directory
(e.g. `core.qemu-arm64-release` built from public open-source product targets)
to ensure realistic binary content without including private/internal code.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys

# Try importing lz4.block, print friendly error if missing
try:
    import lz4.block
except ImportError:
    print(
        "Error: python 'lz4' module is required. Install with 'pip install lz4'."
    )
    sys.exit(1)

CHUNK_SIZE = 128 * 1024  # 128 KiB
SUB_CHUNK_SIZE = 32 * 1024  # 32 KiB
NUM_SUB_CHUNKS = 4


def get_build_dir():
    fuchsia_root = os.path.normpath(
        os.path.join(os.path.dirname(__file__), "../../../../..")
    )
    fx_path = os.path.join(fuchsia_root, "scripts/fx")
    try:
        build_dir = subprocess.check_output(
            [fx_path, "get-build-dir"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        return build_dir
    except subprocess.CalledProcessError:
        print(
            "Error: Could not determine build directory using 'fx get-build-dir'."
        )
        sys.exit(1)


def compress_lz4_block(data):
    # Matches delivery_blob's LZ4 HC level 12 (custom(12))
    # store_size=False to get raw compressed payload size
    try:
        compressed = lz4.block.compress(
            data, mode="high_compression", compression=12, store_size=False
        )
        return len(compressed)
    except Exception as e:
        print(f"LZ4 compress failed: {e}", file=sys.stderr)
        return None


def compress_lz4(data):
    total_len = 0
    sub_ratios = []
    for i in range(NUM_SUB_CHUNKS):
        sub_chunk = data[i * SUB_CHUNK_SIZE : (i + 1) * SUB_CHUNK_SIZE]
        compressed_len = compress_lz4_block(sub_chunk)
        if compressed_len is None:
            return None, None
        total_len += compressed_len
        sub_ratios.append(compressed_len / SUB_CHUNK_SIZE)
    return total_len, sub_ratios


def compress_zstd_block(data):
    # Matches delivery_blob's ZSTD level 14
    try:
        proc = subprocess.run(
            ["zstd", "-14", "-c", "-q"],
            input=data,
            capture_output=True,
            check=True,
        )
        return len(proc.stdout)
    except Exception as e:
        print(f"ZSTD compress failed: {e}", file=sys.stderr)
        return None


def compress_zstd(data):
    total_len = 0
    sub_ratios = []
    for i in range(NUM_SUB_CHUNKS):
        sub_chunk = data[i * SUB_CHUNK_SIZE : (i + 1) * SUB_CHUNK_SIZE]
        compressed_len = compress_zstd_block(sub_chunk)
        if compressed_len is None:
            return None, None
        total_len += compressed_len
        sub_ratios.append(compressed_len / SUB_CHUNK_SIZE)
    return total_len, sub_ratios


def score_match(sub_ratios, target):
    # Require each 32 KiB sub-chunk to actually compress (< 0.98) so no chunk
    # falls back to uncompressed memcpy during delivery blob decompression.
    if any(r >= 0.98 for r in sub_ratios):
        return float("inf")
    avg_ratio = sum(sub_ratios) / len(sub_ratios)
    max_sub_diff = max(abs(r - target) for r in sub_ratios)
    return abs(avg_ratio - target) + max_sub_diff


def main():
    parser = argparse.ArgumentParser(
        description="Extract realistic 128 KiB test blobs."
    )
    parser.add_argument(
        "--build-dir",
        help="Path to Fuchsia build directory (defaults to fx get-build-dir)",
        default=None,
    )
    args = parser.parse_args()

    if not shutil.which("zstd"):
        print("Error: 'zstd' executable is required and was not found in PATH.")
        sys.exit(1)

    script_dir = os.path.dirname(os.path.abspath(__file__))
    build_dir = args.build_dir or get_build_dir()
    all_blobs_json = os.path.join(build_dir, "all_blobs.json")
    output_dir = os.path.normpath(os.path.join(script_dir, "../test_data"))

    if not os.path.exists(all_blobs_json):
        print(
            f"Error: {all_blobs_json} does not exist. Ensure the target is built."
        )
        sys.exit(1)

    print(f"Using build directory: {build_dir}")
    print(f"Output directory: {output_dir}")

    with open(all_blobs_json, "r") as f:
        blobs = json.load(f)

    targets = [0.40, 0.55, 0.70]

    # Store the best match for each (algo, target)
    # Key: (algo, target), Value: (diff, file_path, offset, chunk_data, ratio)
    best_matches = {}
    for algo in ["lz4", "zstd"]:
        for target in targets:
            best_matches[(algo, target)] = (
                float("inf"),
                None,
                None,
                None,
                None,
            )

    print(f"Scanning {len(blobs)} blobs for realistic 128 KiB chunks...")

    scanned_files = 0
    for entry in blobs:
        source_path = entry.get("source_path")
        if not source_path or "test_data/" in source_path:
            continue

        abs_path = os.path.normpath(os.path.join(build_dir, source_path))
        if not os.path.exists(abs_path):
            continue

        size = entry.get("size", 0)
        if size < CHUNK_SIZE:
            continue

        try:
            with open(abs_path, "rb") as f:
                data = f.read()
        except Exception:
            continue

        num_chunks = len(data) // CHUNK_SIZE
        for i in range(num_chunks):
            chunk = data[i * CHUNK_SIZE : (i + 1) * CHUNK_SIZE]

            # Compress and measure
            lz4_len, lz4_sub_ratios = compress_lz4(chunk)
            zstd_len, zstd_sub_ratios = compress_zstd(chunk)

            if lz4_len is None or zstd_len is None:
                continue

            lz4_ratio = lz4_len / CHUNK_SIZE
            zstd_ratio = zstd_len / CHUNK_SIZE

            # Check LZ4 matches
            for target in targets:
                diff = score_match(lz4_sub_ratios, target)
                if diff < best_matches[("lz4", target)][0]:
                    best_matches[("lz4", target)] = (
                        diff,
                        source_path,
                        i * CHUNK_SIZE,
                        chunk,
                        lz4_ratio,
                    )

            # Check ZSTD matches
            for target in targets:
                diff = score_match(zstd_sub_ratios, target)
                if diff < best_matches[("zstd", target)][0]:
                    best_matches[("zstd", target)] = (
                        diff,
                        source_path,
                        i * CHUNK_SIZE,
                        chunk,
                        zstd_ratio,
                    )

        scanned_files += 1
        if scanned_files % 100 == 0:
            print(f"Scanned {scanned_files} files...")

    # Write out the best chunks
    os.makedirs(output_dir, exist_ok=True)
    print("\nExtraction Results:")
    for (algo, target), (
        diff,
        file_path,
        offset,
        chunk_data,
        ratio,
    ) in best_matches.items():
        if chunk_data is None:
            print(
                f"Error: No candidate found for {algo.upper()} at target {target*100}%"
            )
            continue

        target_name = f"{algo}_{int(target*100)}.bin"
        output_path = os.path.join(output_dir, target_name)

        with open(output_path, "wb") as out_f:
            out_f.write(chunk_data)

        print(
            f"Saved {target_name:12} (Ratio: {ratio:.3f}, Target: {target*100}%)"
        )
        print(f"  Source: {file_path} at offset {offset} (diff: {diff:.5f})")


if __name__ == "__main__":
    main()
