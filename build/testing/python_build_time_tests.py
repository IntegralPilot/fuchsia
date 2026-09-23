#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Run a series of Python tests from a given directory or explicit list. A.k.a a basic pytest."""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source-dir",
        type=Path,
        help="Source directory to scan for *_test.py files to run.",
    )
    parser.add_argument(
        "--test-files",
        type=Path,
        nargs="+",
        default=[],
        help="Python test file path.",
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="Do not print anything, except errors.",
    )
    parser.add_argument(
        "--stamp", type=Path, help="Stamp file to write on success."
    )
    parser.add_argument(
        "--library-infos",
        type=Path,
        help="Path to the library infos JSON file. Requires --depfile.",
    )
    parser.add_argument(
        "--depfile",
        type=Path,
        help="Path to the depfile to generate. Requires --stamp.",
    )

    args = parser.parse_args()

    if args.library_infos:
        if not args.depfile:
            parser.error("--depfile is required if --library-infos is provided")
        if not args.stamp:
            parser.error(
                "--stamp is required if --library-infos is provided to use as depfile target"
            )

        with open(args.library_infos, "r") as f:
            lib_infos = json.load(f)

        dep_files = []
        for lib_info in lib_infos:
            source_root = lib_info["source_root"]
            for source in lib_info["sources"]:
                dep_files.append(os.path.join(source_root, source))

        dep_files.append(str(args.library_infos))

        args.depfile.write_text(
            "{}: {}\n".format(args.stamp, " ".join(dep_files))
        )

    test_files = args.test_files
    if args.source_dir:
        source_dir = args.source_dir
        test_files += sorted(
            (source_dir / filename)
            for filename in os.listdir(args.source_dir)
            if filename.endswith("_test.py")
        )

    failures = []
    for test_file in test_files:
        if not args.quiet:
            print(f"Running {test_file}", file=sys.stderr)
        ret = subprocess.run(
            [sys.executable, "-S", test_file],
            text=True,
            capture_output=True,
        )
        if ret.returncode != 0:
            print(
                f"FAILURE: STDOUT -----\n{ret.stdout}\nSTDERR -----------\n{ret.stderr}\n"
            )
            failures.append(str(test_file))

    count = len(test_files)

    if failures:
        print(
            "ERROR: %s tests out of %s failed!\n%s\n"
            % (len(failures), count, "\n".join(failures)),
            file=sys.stderr,
        )
        return 1

    if not args.quiet:
        print(f"SUCCESS: {count} tests passed.")

    if args.stamp:
        args.stamp.write_text("ok")

    return 0


if __name__ == "__main__":
    sys.exit(main())
