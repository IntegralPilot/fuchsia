#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys
import zipfile

"""Create a zip file containing the markdown files listed in the source-file-manifest parameter."""


def main():
    parser = argparse.ArgumentParser()

    parser.add_argument(
        "--source-file-manifest",
        help="A path to a manifest of files to include in the archive in the form of a JSON list.",
        required=True,
    )
    parser.add_argument(
        "--output",
        help="Path at which to write the archive in zip format.",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        help="Path at which to write the depfile.",
        required=True,
    )
    args = parser.parse_args()

    with open(args.source_file_manifest) as f:
        srclist = json.load(f)

    with open(args.depfile, "w") as depfile:
        depfile.write(f"{args.output}: ")
        with zipfile.ZipFile(args.output, "w") as file:
            for src in srclist:
                file.write(src, os.path.basename(src))
                depfile.write(f"{src} ")
        depfile.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
