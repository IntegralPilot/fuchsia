#!/usr/bin/env fuchsia-vendored-python

# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import shutil
import sys
from pathlib import Path


def escape_path(path: Path | str) -> str:
    return str(path).replace(" ", "\\ ")


def main():
    params = argparse.ArgumentParser(
        description="Copy all files in a directory tree and touch a stamp file"
    )
    params.add_argument("source", type=Path)
    params.add_argument("target", type=Path)
    params.add_argument("stamp", type=Path)
    params.add_argument("--ignore_patterns", nargs="+")
    params.add_argument("--depfile", nargs=1, type=Path)
    args = params.parse_args()

    if args.target.is_file():
        args.target.unlink()
    if args.target.is_dir():
        shutil.rmtree(args.target, ignore_errors=True)

    ignore = None
    if args.ignore_patterns:
        ignore = shutil.ignore_patterns(*args.ignore_patterns)

    file_list = [args.source]

    def ignore_wrapper(current, children):
        nonlocal file_list, ignore
        to_ignore = set(ignore(current, children)) if ignore else set()
        file_list.extend(
            (Path(current) / f) for f in children if f not in to_ignore
        )
        return to_ignore

    shutil.copytree(
        args.source,
        args.target,
        symlinks=True,
        ignore=ignore_wrapper,
    )

    if args.depfile:
        os.makedirs(os.path.dirname(args.depfile[0]), exist_ok=True)
        with open(args.depfile[0], "w") as f:
            f.write(escape_path(args.stamp) + ": ")
            f.write(" ".join(escape_path(file) for file in file_list))
            f.write("\n")
            for file in file_list:
                target_path = os.path.join(
                    args.target, os.path.relpath(file, start=args.source)
                )
                print(
                    escape_path(target_path) + ": " + escape_path(file),
                    file=f,
                )

    args.stamp.touch()


if __name__ == "__main__":
    sys.exit(main())
