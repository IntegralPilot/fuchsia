#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(__file__) + "/../../../build/python/modules")
from depfile import DepFile


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Copy kernel ZBI based on metadata"
    )
    parser.add_argument(
        "--metadata",
        required=True,
        help="Path to the kernel metadata JSON file",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path where the kernel ZBI should be copied",
    )
    parser.add_argument(
        "--depfile",
        required=True,
        help="Path to write the dependency file to",
    )
    args = parser.parse_args()

    with open(args.metadata, "r") as f:
        metadata = json.load(f)

    if not metadata:
        print("Error: metadata is empty", file=sys.stderr)
        return 1

    # The `zbi` entry in the `kernel_aib_input` GN metadata.
    kernel_path = metadata[0].get("zbi")
    if not kernel_path:
        print("Error: 'zbi' not found in metadata", file=sys.stderr)
        return 1

    # Copy the file
    shutil.copy(kernel_path, args.output)

    with open(args.depfile, "w") as f:
        DepFile.from_deps(args.output, [kernel_path]).write_to(f)

    return 0


if __name__ == "__main__":
    sys.exit(main())
