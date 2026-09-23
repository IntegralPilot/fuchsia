#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generates a weights file from a ninja log.

This script reads a ninja .ninja_log file and extracts build durations for each
target. It filters out durations below a specified minimum and writes the
results to a weights file, sorted by duration in descending order.
"""

import argparse
import os
import sys
from pathlib import Path


def convert_log_to_weights(
    ninjalog_path: Path,
    weights_path: Path,
    minimum_duration_ms: int,
) -> None:
    weights: dict[str, int] = {}

    last_stop = 0
    with open(ninjalog_path) as logfile:
        for linenum, line in enumerate(logfile.readlines()):
            if line.startswith("#"):
                continue
            tokens = line.split("\t")
            start = int(tokens[0])
            stop = int(tokens[1])
            path = tokens[3]

            # Make sure that the file is in order, and not from an incremental
            # build.
            if stop < last_stop:
                raise ValueError(
                    f"File appears to contain entries from multiple builds, this is an error.  Use a log file from a single, clean, build. {last_stop} {stop} {path}"
                )
            last_stop = stop

            if path in weights:
                raise ValueError(
                    f"Found duplicate entries for path: {path} at line {linenum+1}"
                )
            duration = stop - start
            if duration > minimum_duration_ms:
                weights[path] = stop - start

    print(f"read {len(weights)} weights from ninja log")

    os.makedirs(weights_path.parent, exist_ok=True)
    with open(weights_path, "w") as weights_file:
        for path, weight in sorted(
            weights.items(), key=lambda x: x[1], reverse=True
        ):
            print(f"{path},{weight}", file=weights_file)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--ninja-log",
        type=Path,
        required=True,
        help="Path to a ninja log file, it must only contain a SINGLE build's output",
    )
    parser.add_argument(
        "--weights-file",
        type=Path,
        required=True,
        help="Path to a file to write ninja weights to.",
    )
    parser.add_argument(
        "--minimum-duration-ms",
        type=int,
        default=60000,
        help="Minimum duration in ms to use as a weight.",
    )
    args = parser.parse_args()

    print(f"reading ninja log: {args.ninja_log}")
    convert_log_to_weights(
        args.ninja_log, args.weights_file, args.minimum_duration_ms
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
