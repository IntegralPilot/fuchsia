#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Merges the contents of multiple ninja weights csv files

This script reads multiple weights csv files created by the ninjalog_to_weights.py
script, and then merges them, taking the longest duration for any path that appears
in more than one file.
"""

import argparse
import os
import sys
from pathlib import Path


def merge_weights_files_finding_max(
    weights_files: list[Path], merged_weights_file: Path
) -> None:
    """Merge multiples weights files into one that contains the max value for each path.

    This merges multiple weights files into a single file, using the
    max value from all files for each path that's found.
    """
    merged_weights: dict[str, int] = {}

    # Open each weights file, and add to the merged weights if the
    # value is larger than the existing value
    for weights_file_path in weights_files:
        for line in weights_file_path.read_text().splitlines():
            # each line is a csv line of `<path>,<weight>`
            path, _, weight_str = line.partition(",")
            weight = int(weight_str)

            existing_weight = merged_weights.get(path)
            if not existing_weight or weight > existing_weight:
                merged_weights[path] = weight

    os.makedirs(merged_weights_file.parent, exist_ok=True)
    with open(merged_weights_file, "w") as weights_file:
        for path, weight in sorted(
            merged_weights.items(), key=lambda x: x[1], reverse=True
        ):
            print(f"{path},{weight}", file=weights_file)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--weights-files",
        type=Path,
        nargs="+",
        required=True,
        help="Paths of ninja weights files to merge",
    )
    parser.add_argument(
        "--merged-weights-file",
        type=Path,
        required=True,
        help="Path to a file to write ninja weights to.",
    )
    args = parser.parse_args()
    merge_weights_files_finding_max(
        args.weights_files, args.merged_weights_file
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
