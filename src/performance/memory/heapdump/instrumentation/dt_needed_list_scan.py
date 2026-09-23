#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import re
import subprocess
import sys

PATTERN = re.compile(r"\(NEEDED\) +Shared library: \[(.*)\]$")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--input",
        help="Path of the ELF library to be inspected",
        required=True,
    )
    parser.add_argument(
        "--output",
        type=argparse.FileType("w"),
        help="Detected DT_NEEDED entries, one per line",
        required=True,
    )
    parser.add_argument("readelf", help="llvm-readelf binary")
    args = parser.parse_args()

    readelf_output = subprocess.check_output(
        [args.readelf, "-d", args.input],
        encoding="ascii",
    )
    for line in readelf_output.splitlines():
        if m := PATTERN.search(line):
            args.output.write(m.group(1) + "\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
