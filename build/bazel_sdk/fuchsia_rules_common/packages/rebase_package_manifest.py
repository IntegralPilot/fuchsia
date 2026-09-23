# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Rebase paths in package manifest to be relative to artifact_base_path."""

import argparse
import json
import os


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--package-manifest",
        help="original package manifest file",
        required=True,
    )
    parser.add_argument(
        "--updated-package-manifest",
        help="output package manifest file",
        required=True,
    )
    return parser.parse_args()


def main():
    args = parse_args()

    with open(args.package_manifest) as f:
        package_manifest_json = json.load(f)

    # Package manifests have an optional field `'blob_sources_relative'`, which
    # states whether or not the files referenced in the manifest are relative to
    # some working directory (with the value `working_dir`), or to the directory
    # that contains the package manifest (which is the value `file`). When we're
    # `file`, we will need to first make the paths relative to the manifest
    # directory, before we make it relative to the `file` path.
    blob_sources_relative = package_manifest_json.get(
        "blob_sources_relative", "working_dir"
    )

    # The output manifest contents will always be relative to the manifest.
    package_manifest_json["blob_sources_relative"] = "file"
    package_manifest_dir = os.path.dirname(args.package_manifest)
    updated_package_manifest_dir = os.path.dirname(
        args.updated_package_manifest
    )

    for blob in package_manifest_json["blobs"]:
        source_path = blob["source_path"]

        if blob_sources_relative == "file":
            source_path = os.path.join(package_manifest_dir, source_path)

        blob["source_path"] = os.path.relpath(
            source_path, updated_package_manifest_dir
        )

    try:
        subpackages = package_manifest_json["subpackages"]
    except KeyError:
        pass
    else:
        for subpackage in subpackages:
            manifest_path = subpackage["manifest_path"]

            if blob_sources_relative == "file":
                manifest_path = os.path.join(
                    package_manifest_dir, manifest_path
                )

            subpackage["manifest_path"] = os.path.relpath(
                manifest_path, updated_package_manifest_dir
            )

    with open(args.updated_package_manifest, "w") as f:
        json.dump(package_manifest_json, f, indent=2)


if __name__ == "__main__":
    main()
