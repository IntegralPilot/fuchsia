#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
This generates an input linker script to override LINUXBOOT_INIT_SIZE.
It was set to LINUXBOOT32_INIT_SIZE in linuxboot32.ld via PROVIDE_HIDDEN.
For a linuxbiboot image, this must be the maximum of LINUXBOOT32_INIT_SIZE
and the LINUXBOOT_INIT_SIZE value that linuxboot64.ld would have used, i.e.
the whole memory image size (including .bss) of the inner 64-bit executable.
That size is computed from the llvm-readelf --sections JSON output as the
maximum end address of any SHF_ALLOC section.
"""

import argparse
import json
import os
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=argparse.FileType("w"),
        help="Output file (input linker script)",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        type=argparse.FileType("w"),
        required=True,
    )
    parser.add_argument(
        "--rspfile",
        type=argparse.FileType("r"),
        help="Response file containing path to linuxbiboot64 ELF binary",
        required=True,
    )
    parser.add_argument("readelf", help="llvm-readelf binary", nargs=1)
    args = parser.parse_args()

    [elf_file] = [line.strip() for line in args.rspfile.readlines()]
    args.rspfile.close()

    args.depfile.write(f"{args.output.name}: {elf_file}\n")
    args.depfile.close()

    [file_data] = json.loads(
        subprocess.check_output(
            [
                args.readelf[0],
                "--elf-output-style=JSON",
                "--sections",
                elf_file,
            ]
        )
    )

    def alloc_end(section):
        section = section["Section"]
        flags = [flag["Name"] for flag in section["Flags"]["Flags"]]
        if "SHF_ALLOC" not in flags:
            return 0
        return section["Address"] + section["Size"]

    end = max(alloc_end(section) for section in file_data["Sections"])

    args.output.write(
        f"""
/* Generated from {elf_file} by {os.path.relpath(__file__)}. DO NOT EDIT! */
HIDDEN(LINUXBOOT64_INIT_SIZE = Linux64Entry + {end} - LINUXBOOT_LOAD_ADDRESS);
HIDDEN(LINUXBOOT_INIT_SIZE = MAX(LINUXBOOT32_INIT_SIZE, LINUXBOOT64_INIT_SIZE));
"""
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
