#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import sys


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate a hermetic inputs file for ffx_action"
    )
    parser.add_argument(
        "--additional-hermetic-inputs",
        type=argparse.FileType("r"),
        help="The path to another hermetic inputs file to merge in",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="The path to the hermetic inputs file to write",
    )
    args = parser.parse_args()

    inputs = []
    if args.additional_hermetic_inputs:
        additional_inputs = args.additional_hermetic_inputs.readlines()
        additional_inputs = [i.strip() for i in additional_inputs]
        inputs.extend(additional_inputs)
    with open(args.output, "w") as output:
        for input in inputs:
            output.write(input + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
