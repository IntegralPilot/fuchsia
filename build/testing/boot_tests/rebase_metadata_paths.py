#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys

USAGE = """Rebases the 'path' fields within a GN JSON metadata file, assuming
the contents are a list of entries with a top-level "path" field, and that path
value is relative to the build directory.
"""


def main():
    parser = argparse.ArgumentParser(usage=USAGE)
    parser.add_argument(
        "--root-build-dir",
        help="Path to the root of the build directory (i.e., what metadata paths are currently relative to)",
        required=True,
    )
    parser.add_argument(
        "--new-base",
        help="New base path to rebase metadata paths against",
        required=True,
    )
    parser.add_argument(
        "--metadata",
        metavar="FILE",
        help="Path at which to find the image metadata JSON file",
        required=True,
    )
    parser.add_argument(
        "--output",
        metavar="FILE",
        help="Path at which to output the metadata file with rebases paths",
        required=True,
    )

    args = parser.parse_args()
    with open(args.metadata) as f:
        metadata = json.load(f)

    for idx, _ in enumerate(metadata):
        path = os.path.join(args.root_build_dir, metadata[idx]["path"])
        metadata[idx]["path"] = os.path.relpath(path, args.new_base)

    # wrap the image vector to a map
    contents = {}
    contents["images"] = metadata
    contents["board_name"] = "boot_test"
    with open(args.output, "w") as f:
        json.dump(contents, f, indent=4)

    return 0


if __name__ == "__main__":
    sys.exit(main())
