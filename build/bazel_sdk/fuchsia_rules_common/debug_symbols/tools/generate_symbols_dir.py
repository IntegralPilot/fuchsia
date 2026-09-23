#!/usr/bin/env python3
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Generates debug symbols directory from a list of ids.txt files.

The input files are a list of single-line ids.txt files in the format of
"<build ID> <object_file_with_symbols>"

This script should only be invoked by _fuchsia_package_impl in package.bzl.
"""
import argparse
import os
import shutil
import sys

from debug_symbols.tools.pylib import elf_info

_STAMP_FILE_NAME = ".stamp"


def create_empty(path: str):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write("")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--debug", action="store_true", help="Enable debug logs."
    )
    parser.add_argument("output_dir", help="Output directory path.")
    parser.add_argument(
        "input_file", nargs="*", help="Input ids.txt file paths."
    )

    args = parser.parse_args()

    log_func = None
    if args.debug:
        log_func = lambda msg: print("LOG " + msg, file=sys.stderr)

    # Read all input files, one line at a time.
    # Each line is: BUILD_ID  ELF_WITH_SYMBOLS_FILE_PATH
    # and stores the result in a { BUILD_ID -> ELF_FILE } map.
    debug_map = {}
    for input_file in args.input_file:
        with open(input_file) as f:
            for line in f:
                items = line.strip().split()
                if len(items) != 2:
                    print(
                        "WARNING: Invalid input line: [%s]" % line,
                        file=sys.stderr,
                    )
                    continue

                build_id = items[0]
                elf_with_symbols = items[1]
                if build_id in debug_map:
                    if debug_map[build_id] == elf_with_symbols:
                        # Ignore duplicate entries
                        continue
                    print(
                        "ERROR: Duplicate entries for build-id %s\n  %s\n  %s\n"
                        % (build_id, debug_map[build_id], elf_with_symbols),
                        file=sys.stderr,
                    )
                    return 1
                debug_map[build_id] = elf_with_symbols

    copied_files = []

    # Check source files for symbols and debug_info
    for build_id, elf_file in debug_map.items():
        build_id_path = os.path.join(
            args.output_dir, build_id[:2], build_id[2:]
        )
        debug_path = build_id_path + ".debug"
        elf = elf_info.ElfInput.open_file(
            elf_file, log_func=log_func
        ).ensure_elf64()
        if not elf:
            print(f"ERROR: Not an ELF64 file: {elf_file}", file=sys.stderr)
            return 1

        if elf.is_stripped():
            if False:  # Change to True for debugging
                print(
                    f"WARNING: binary is already stripped: {elf_file}",
                    file=sys.stderr,
                )
            continue

        # If the file is not stripped, add it to the output no matter whether
        # it has debug_info.
        os.makedirs(os.path.dirname(debug_path), exist_ok=True)
        shutil.copy2(elf_file, debug_path)
        copied_files.append(build_id)

    if len(copied_files) == 0:
        # Create an empty file if the output directory is empty, as bazel will ignore
        # empty directories
        create_empty(os.path.join(args.output_dir, ".ensure_there_is_one_file"))

    with open(os.path.join(args.output_dir, _STAMP_FILE_NAME), "w") as f:
        f.write("\n".join(copied_files))

    return 0


if __name__ == "__main__":
    sys.exit(main())
