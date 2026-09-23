#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Copy individual subtarget outputs from a bazel_build_group() set of outputs."""

import argparse
import filecmp
import os
import shutil
import sys
from pathlib import Path


class DirectoryAction(argparse.Action):
    def __init__(  # type: ignore
        self, option_strings, dest, nargs=None, default=None, **kwargs
    ):
        super().__init__(option_strings, dest, nargs="+", default=[], **kwargs)

    def __call__(self, parser, namespace, values, option_string):  # type: ignore
        if len(values) < 3:
            raise ValueError(
                f"expected at least 3 arguments for {option_string}"
            )
        dest_list = getattr(namespace, self.dest, [])
        src_dir = Path(values[0])
        dst_dir = Path(values[1])
        tracked_files = values[2:]
        dest_list.append((src_dir, dst_dir, tracked_files))
        setattr(namespace, self.dest, dest_list)


class FileAction(argparse.Action):
    def __init__(  # type: ignore
        self, option_strings, dest, nargs=None, default=None, **kwargs
    ):
        super().__init__(option_strings, dest, nargs="+", default=[], **kwargs)

    def __call__(self, parser, namespace, values, option_string):  # type: ignore
        if len(values) & 1 != 0:
            raise ValueError(
                f"expected an even number of file arguments, got {len(values)}"
            )
        dest_list = getattr(namespace, self.dest, [])
        dest_list.extend(
            (Path(src_file), Path(dst_file))
            for src_file, dst_file in zip(values[::2], values[1::2])
        )
        setattr(namespace, self.dest, dest_list)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)

    parser.add_argument(
        "--directory",
        action=DirectoryAction,
        dest="directories",
        help="An input / output directory path pair followed by at least one relative tracked file path",
    )

    parser.add_argument(
        "--files",
        action=FileAction,
        help="Pairs of input / output file paths to copy.",
    )

    args = parser.parse_args()

    for src_dir, dst_dir, tracked_files in args.directories:
        update_needed = False
        for tracked in tracked_files:
            src_file = src_dir / tracked
            dst_file = dst_dir / tracked

            if not os.path.lexists(dst_file):
                # destination file does not exist, update is necessary.
                update_needed = True
                break

            if src_file.lstat().st_mtime != dst_file.lstat().st_mtime:
                # destination and source files have different mtimes, update is
                # needed.
                #
                # NOTE: The __mtime__, instead of __content__, of tracked files
                # are used to represent freshness of directory outputs.
                #
                # See https://fxbug.dev/359710469.
                update_needed = True
                break

        if update_needed:
            shutil.rmtree(dst_dir)
            shutil.copytree(src_dir, dst_dir, copy_function=os.link)

    for src_file, dst_file in args.files:
        if dst_file.exists() and filecmp.cmp(src_file, dst_file, shallow=False):
            continue

        if os.path.lexists(dst_file):
            if dst_file.is_dir():
                shutil.rmtree(dst_file)
            else:
                dst_file.unlink()

        dst_file.parent.mkdir(parents=True, exist_ok=True)
        dst_file.hardlink_to(src_file)

    return 0


if __name__ == "__main__":
    sys.exit(main())
