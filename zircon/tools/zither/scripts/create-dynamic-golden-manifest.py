#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys

# Create a dynamic manifest of golden comparisons to pass to golden_files().


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--source-dir",
        help="A source directory under which to register goldens",
        required=True,
    )
    parser.add_argument(
        "--candidates",
        help="A path to a manifest of golden candidates in the form of a JSON list",
        required=True,
    )
    parser.add_argument(
        "--output",
        help="Path at which to write a golden comparison JSON manifest (a la golden_files())",
        required=True,
    )
    args = parser.parse_args()

    with open(args.candidates) as f:
        candidates = json.load(f)

    comparisons = [
        dict(
            candidate=candidate,
            golden=os.path.join(args.source_dir, os.path.basename(candidate)),
        )
        for candidate in candidates
    ]

    with open(args.output, "w") as output:
        json.dump(comparisons, output, indent=4)

    return 0


if __name__ == "__main__":
    sys.exit(main())
