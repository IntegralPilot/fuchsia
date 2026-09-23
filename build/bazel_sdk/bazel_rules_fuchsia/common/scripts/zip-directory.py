#!/usr/bin/env python3
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Create a zip archive. Used as a portable alternative to the host `zip` tool."""

import argparse
import os
import sys
import zipfile

_DEFAULT_COMPRESSION_LEVEL = 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_file", help="Output file path.")
    parser.add_argument(
        "source_dir",
        help="Source directory. All files in it will be added to the archive.",
    )
    parser.add_argument(
        "--compression-level",
        help=f"Compression level, default is {_DEFAULT_COMPRESSION_LEVEL}",
    )

    args = parser.parse_args()

    # Get compression level.
    compression_level = _DEFAULT_COMPRESSION_LEVEL
    if args.compression_level:
        try:
            compression_level = int(args.compression_level)
        except ValueError:
            compression_level = -1
        if compression_level < 0 or compression_level > 9:
            parser.error(
                f"Invalid compression level {args.compression_level}, valid values are 0 to 9!"
            )

    # Get source files list.
    source_files = []
    for root, dirs, files in os.walk(args.source_dir):
        source_files.extend(
            os.path.join(root, f)
            for f in files
            # exclude cache files for determinism
            if not ("__pycache__" in root or f.endswith(".pyc"))
        )

    # Create zip archive.
    source_dir_prefix = args.source_dir
    if not source_dir_prefix.endswith("/"):
        source_dir_prefix += "/"

    zip_compression = (
        zipfile.ZIP_STORED if compression_level == 0 else zipfile.ZIP_DEFLATED
    )
    with zipfile.ZipFile(
        args.output_file,
        mode="w",
        compression=zip_compression,
        compresslevel=compression_level,
    ) as zip_out:
        for src_file in source_files:
            assert src_file.startswith(
                source_dir_prefix
            ), "Invalid source file %s does not start with %s" % (
                src_file,
                source_dir_prefix,
            )

            # Create zip entry with fixed mtime. The ZipInfo() constructor
            # uses 1980-1-1 0:0;0 by default.
            dst_file = src_file[len(source_dir_prefix) :]
            dst_entry = zipfile.ZipInfo(dst_file)

            with open(src_file, "rb") as f:
                src_data = f.read()

            zip_out.writestr(dst_entry, src_data)

    return 0


if __name__ == "__main__":
    sys.exit(main())
