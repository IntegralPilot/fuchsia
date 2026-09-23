#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Strips an input ELF binary and produce a single-line ids.txt file

The second output file will be a single-line ids.txt file in the format of
"<elf_with_symbols_file> <debug_file>".

If the source file is not an ELF file or does not contain a build ID, the
output file will be empty.

This script should only be invoked by fuchsia_package_impl in
fuchsia_package.bzl.
"""

import argparse
import os
import shutil
import subprocess
import sys

from debug_symbols.tools.pylib import elf_info


def debug(msg: str):
    # uncomment the line below to enable debug messages.
    # print('LOG: ' + msg, file=sys.stderr)
    pass


def write_file(path: str, content: str):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)


def create_empty(path: str):
    write_file(path, "")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--debug", action="store_true", default=False, help="Enable debug logs"
    )
    parser.add_argument("objcopy", help="Path to objcopy host tool.")
    parser.add_argument("elf_with_symbols_file", help="Input ELF file path.")
    parser.add_argument("elf_stripped", help="Output ELF file path.")
    parser.add_argument("ids_txt", help="Output ids.txt file path.")

    args = parser.parse_args()

    log_func = None
    if args.debug:
        log_func = lambda msg: print("LOG " + msg, file=sys.stderr)

    # If the file is not an ELF, e.g. a font file or an image, or if
    # this file has no symbols to strip, then we just copy it as-is
    # and create an empty ids.txt output file.
    elf_file = elf_info.ElfInput.open_file(
        args.elf_with_symbols_file, log_func=log_func
    ).ensure_elf64()
    if not elf_file or not elf_file.has_sections():
        shutil.copy2(args.elf_with_symbols_file, args.elf_stripped)
        create_empty(args.ids_txt)
        return 0

    # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
    # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
    # inheritance which can confuse child Python processes.
    env = dict(os.environ)
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)

    # Strip symbols from the ELF.
    subprocess.check_call(
        [
            args.objcopy,
            "--strip-all",
            args.elf_with_symbols_file,
            args.elf_stripped,
        ],
        env=env,
    )

    # Get Build ID.
    build_id = elf_file.get_build_id()
    if not build_id:
        print(
            "WARNING: No build id in ELF: " + args.elf_with_symbols_file,
            file=sys.stderr,
        )
        create_empty(args.ids_txt)
        return 0

    debug("FOUND BUILD_ID [%s] for %s" % (build_id, args.elf_with_symbols_file))
    write_file(args.ids_txt, "%s %s\n" % (build_id, args.elf_with_symbols_file))
    return 0


if __name__ == "__main__":
    sys.exit(main())
