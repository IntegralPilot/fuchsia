#!/usr/bin/env fuchsia-vendored-python
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import re
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--headers",
        help="The list of header files to check",
        default=[],
        nargs="*",
    )
    parser.add_argument(
        "--stamp",
        help="The path to the stamp file in case of success",
        required=True,
    )
    args = parser.parse_args()

    has_errors = False

    matcher = re.compile(r"^#pragma once", re.MULTILINE)
    for header in args.headers:
        with open(header, "r") as header_file:
            if matcher.search(header_file.read()):
                print(f"Error: pragma disallowed in SDK: {header}")
                has_errors = True
    if has_errors:
        return 1

    with open(args.stamp, "w") as stamp_file:
        stamp_file.write("Success!")
    return 0


if __name__ == "__main__":
    sys.exit(main())
