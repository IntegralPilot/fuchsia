#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Script to extract line-separated text files with contents from create-system's images.json.
"""

import argparse
import json
import os
import sys


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Parse assembly create-system output manifest to get package info."
    )
    parser.add_argument(
        "--package-set",
        type=str,
        required=True,
        choices=["base", "cache"],
        help="The package set for which to emit metadata.",
    )
    parser.add_argument("--contents", type=str, choices=["name", "manifest"])
    parser.add_argument(
        "--images-manifest",
        type=argparse.FileType("r"),
        required=True,
        help="Path to images.json created by `ffx assembly create-system`.",
    )
    parser.add_argument(
        "--output",
        type=argparse.FileType("w"),
        required=True,
        help="Path to which to write desired output list.",
    )
    args = parser.parse_args()

    images_manifest = json.load(args.images_manifest)
    manifest_path = os.path.dirname(args.images_manifest.name)

    manifest_paths = []
    for image in images_manifest["images"]:
        contents = image.get("contents", dict())
        if "packages" in contents:
            for package in contents["packages"][args.package_set]:
                # Paths in the manifest are relative to the manifest file itself
                manifest_paths.append(
                    # Normalize the paths to remove internal ../../'s.
                    os.path.normpath(
                        os.path.join(manifest_path, package[args.contents])
                    )
                )

    out_package_manifest_list = {
        "content": {"manifests": manifest_paths},
        "version": "1",
    }

    with args.output as output:
        output.write(
            json.dumps(out_package_manifest_list, indent=2, sort_keys=True)
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
