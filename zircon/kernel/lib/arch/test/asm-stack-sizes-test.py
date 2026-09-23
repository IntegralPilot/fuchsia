#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import subprocess
import sys


def expected_stack_sizes(arch):
    # The amount added implicitly by .prologue.fp differs on x86.
    adjust = 8 if arch == "x86_64" else 16
    return sorted(
        [
            sorted({"Functions": ["_start"], "Size": 128 + adjust}.items()),
            sorted({"Functions": ["second"], "Size": 256 + adjust}.items()),
        ]
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--rspfile",
        type=argparse.FileType("r"),
        help="File containing list of ELF file names",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        type=argparse.FileType("w"),
        required=True,
    )
    parser.add_argument(
        "--output",
        type=argparse.FileType("w"),
        help="Output file (success stamp)",
        required=True,
    )
    parser.add_argument("readelf", help="llvm-readelf binary", nargs=1)
    args = parser.parse_args()

    elf_files = [line[:-1] for line in args.rspfile.readlines()]
    args.rspfile.close()

    args.depfile.write(f"{args.output.name}: {' '.join(elf_files)}\n")
    args.depfile.close()

    [file_data] = json.loads(
        subprocess.check_output(
            [
                args.readelf[0],
                "--elf-output-style=JSON",
                "--stack-sizes",
            ]
            + elf_files
        )
    )

    expected = expected_stack_sizes(file_data["FileSummary"]["Arch"])
    sizes = sorted(
        sorted(entry["Entry"].items()) for entry in file_data["StackSizes"]
    )
    assert sizes == expected, f"{sizes=} != {expected=}"

    args.output.write("OK\n")
    args.output.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
