#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generate helpdoc reference docs for fx commands and subcommands.

This script calls fx helpdoc and generates reference docs for fx
commands and subcommands.
"""

import argparse
import os
import subprocess
import sys


def run_fx_helpdoc(
    src_dir: str, out_path: str, dep_file: str, log_to_file: str | None = None
) -> int:
    fx_bin = os.path.join(src_dir, "scripts/fx")
    log_file = None
    if log_to_file:
        log_file = open(log_to_file, "w")
    gen_helpdocs = subprocess.run(
        [fx_bin, "helpdoc", "--depfile", dep_file, "--archive", out_path],
        stdout=log_file,
    )
    if gen_helpdocs.returncode:
        print(gen_helpdocs.stderr)
        return 1

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,  # Prepend helpdoc with this file's docstring.
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-o",
        "--out-path",
        type=str,
        required=True,
        help="Output location where generated docs should go",
    )
    parser.add_argument(
        "-s",
        "--src-dir",
        type=str,
        required=True,
        help="Home location of Fuchsia relative to build",
    )
    parser.add_argument(
        "-l", "--log-to-file", type=str, help="File for logging stdout."
    )
    parser.add_argument("--depfile", type=argparse.FileType("w"), required=True)

    args = parser.parse_args()
    run_fx_helpdoc(
        args.src_dir, args.out_path, args.depfile.name, args.log_to_file
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
