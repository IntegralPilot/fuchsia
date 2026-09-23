#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compare the content of two Bazel SDK directories."""

import argparse
import filecmp
import os
import sys
from pathlib import Path


def get_file_tree_set(root_dir: Path) -> set[Path]:
    """Get a set of all the files reachable from |root_dir|. Does not resolve symlinks."""
    result: set[Path] = set()
    for dirpath, dirnames, filenames in os.walk(root_dir):
        for filename in filenames:
            file = Path(os.path.join(dirpath, filename))
            result.add(file.relative_to(root_dir))
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--first-sdk",
        type=Path,
        required=True,
        help="Path to first SDK directory.",
    )
    parser.add_argument(
        "--second-sdk",
        type=Path,
        required=True,
        help="Path to second SDK directory.",
    )
    parser.add_argument(
        "--stamp-file", type=Path, help="Stamp file to write on success."
    )
    args = parser.parse_args()

    first_sdk = args.first_sdk.resolve()
    second_sdk = args.second_sdk.resolve()

    first_tree = get_file_tree_set(first_sdk)
    second_tree = get_file_tree_set(second_sdk)

    failure = False

    # First compare the simple set of file paths in each SDK.
    # Abort early if they are different.
    missing_files = first_tree - second_tree
    if missing_files:
        print(
            "Missing files from second SDK:\n  %s\n"
            % "  \n".join(sorted(str(f) for f in missing_files))
        )
        failure = True

    extra_files = second_tree - first_tree
    if extra_files:
        print(
            "Extra files in second SDK:\n  %s\n"
            % "  \n".join(sorted(str(f) for f in extra_files))
        )
        failure = True

    if failure:
        return 1

    # Second, compare the content of each file in each SDK.
    # This resolves symlinks.
    for file in first_tree:
        first_file = (first_sdk / file).resolve()
        second_file = (second_sdk / file).resolve()

        if first_file == second_file:
            continue

        assert first_file.exists()
        assert second_file.exists()

        first_stat = first_file.stat()
        second_stat = second_file.stat()
        if first_stat.st_size != second_stat.st_size:
            print(
                "Files with different sizes (%d vs %d): %s"
                % (first_stat.st_size, second_stat.st_size, file)
            )
            failure = True
        elif not filecmp.cmp(first_file, second_file):
            print("Files with same size but different content: %s" % file)
            # Print differences??
            failure = True

    if failure:
        return 1

    if args.stamp_file:
        args.stamp_file.write_text("OK")

    return 0


if __name__ == "__main__":
    sys.exit(main())
