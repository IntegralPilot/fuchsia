#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Collects useful metadata for describing package archives.
For use in the template //build/packages:exported_fuchsia_package_archive."""

import argparse
import json
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()

    def path_arg(validate_type=None):
        def arg(path):
            path = Path(path)
            assert (
                not validate_type
                or validate_type == "file"
                and path.is_file()
                or validate_type == "directory"
                and path.is_dir()
            ), f'Path "{path}" is not a {validate_type}!'
            return path

        return arg

    parser.add_argument(
        "--far",
        help="A path to the package archive.",
        type=path_arg("file"),
        required=True,
    )
    parser.add_argument(
        "--package-manifest",
        help="A path to the underlying package's manifest.",
        type=path_arg("file"),
        required=True,
    )
    parser.add_argument(
        "--cpu",
        help="The underlying package's target cpu architecture.",
        required=True,
    )
    parser.add_argument(
        "--api-level",
        help="The underlying package's target api level.",
        type=int,
        required=True,
    )
    parser.add_argument(
        "--out",
        help="The path to write the json metadata for the package archive.",
        type=path_arg(),
        required=True,
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    package_manifest = json.loads(args.package_manifest.read_text())
    args.out.write_text(
        json.dumps(
            {
                "name": package_manifest["package"]["name"],
                "cpu": args.cpu,
                "api_level": args.api_level,
                "path": str(args.far),
            }
        )
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
