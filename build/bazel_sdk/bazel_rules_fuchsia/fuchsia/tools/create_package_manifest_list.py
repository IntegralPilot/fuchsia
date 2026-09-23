# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Create package manifest file list based on images_config.json."""

import argparse
import json
import os


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--images-config",
        help="images_config.json file path",
        required=True,
    )
    parser.add_argument(
        "--cache-package-manifest-list",
        help="output cache package manifest list file path",
        required=True,
    )
    parser.add_argument(
        "--base-package-manifest-list",
        help="output base package manifest list file path",
        required=True,
    )
    return parser.parse_args()


def write_file(file_path, package_list):
    base = os.path.dirname(file_path)
    package_manifests = [
        os.path.relpath(path, base) + "\n" for path in package_list
    ]
    with open(file_path, "w") as f:
        f.writelines(package_manifests)


def main():
    args = parse_args()
    with open(args.images_config, "r") as f:
        image_config_json = json.load(f)

    write_file(args.base_package_manifest_list, image_config_json["base"])
    write_file(args.cache_package_manifest_list, image_config_json["cache"])


if __name__ == "__main__":
    main()
