#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate a tarmaker input manifest from an input directory containing symlinks.

Really all this does is walk a directory, resolve all symlinks, and output the
contents in the format expected by tarmaker.

NOTE: This assumes that everything in the input directory is intended to go in the
archive! When generating the IDK directory, it's important to ensure no stale
files are carried over between builds.
"""

import argparse
import os
import pathlib


def get_mappings(
    exported_idk_dir: pathlib.Path, cwd: pathlib.Path
) -> dict[pathlib.Path, pathlib.Path]:
    mappings = dict()
    for root_str, dirs, files in os.walk(exported_idk_dir):
        root = pathlib.Path(root_str)

        for dir_name in dirs:
            dir = root / dir_name
            assert not dir.is_symlink(), (
                "Found symlink directory in IDK! This is not allowed: %s" % dir
            )
        for file_name in files:
            file = root / file_name
            src = pathlib.Path(os.path.relpath(file.resolve(strict=True), cwd))
            dest = file.relative_to(exported_idk_dir)

            assert dest not in mappings, (
                "The same file was somehow listed twice in a walk: %s" % file
            )
            mappings[dest] = src
    return mappings


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input-dir",
        type=pathlib.Path,
        help="Path to the input directory.",
        required=True,
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        help="Path to the output tarmaker manifest file",
        required=True,
    )
    args = parser.parse_args()

    mappings = get_mappings(args.input_dir, pathlib.Path.cwd())

    with args.output.open("w") as manifest:
        for dest, src in sorted(mappings.items()):
            manifest.write("%s=%s\n" % (dest, src))


if __name__ == "__main__":
    main()
