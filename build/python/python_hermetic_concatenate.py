#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import shutil
import sys


def main() -> int:
    parser = argparse.ArgumentParser(
        "Create a self-extracting hermetic python binary by concatenating an executable and a ZIP file."
    )

    parser.add_argument(
        "--executable",
        help="Path to executable self extractor",
        required=True,
    )

    parser.add_argument(
        "--zip-file",
        help="Path to the zip file to concatenate",
        required=True,
    )

    parser.add_argument(
        "--output",
        help="Output concatenated file",
        required=True,
    )

    args = parser.parse_args()

    if not os.path.isfile(args.executable):
        print("executable must be a file")
        return 1
    if not os.path.isfile(args.zip_file):
        print("zip-file must be a file")
        return 1

    with open(args.output, "wb") as output_file:
        input_files = [args.executable, args.zip_file]

        for path in input_files:
            with open(path, "rb") as input_file:
                output_file.write(input_file.read())

    # Copy exec bits
    shutil.copymode(args.executable, args.output)

    return 0


if __name__ == "__main__":
    sys.exit(main())
