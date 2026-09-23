#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys

from assembly import FilePath, PackageManifest
from depfile import DepFile
from serialization import json_load


def get_relative_path(relative_path: str, relative_to_file: str) -> str:
    file_parent = os.path.dirname(relative_to_file)
    path = os.path.join(file_parent, relative_path)
    path = os.path.relpath(path, os.getcwd())
    return path


def add_inputs_from_packages(
    package_paths: set[FilePath],
    all_manifest_paths: set[FilePath],
    inputs: list[FilePath],
    in_subpackage: bool = False,
) -> None:
    anonymous_subpackages: set[FilePath] = set()
    for manifest_path in package_paths:
        inputs.append(manifest_path)

        with open(manifest_path, "r") as f:
            package_manifest = json_load(PackageManifest, f)

        for blob in package_manifest.blobs:
            blob_source = str(blob.source_path)

            if package_manifest.blob_sources_relative == "file":
                blob_source = get_relative_path(blob_source, str(manifest_path))

            inputs.append(blob_source)

        for subpackage in package_manifest.subpackages:
            subpackage_path = str(subpackage.manifest_path)

            if package_manifest.blob_sources_relative == "file":
                subpackage_path = get_relative_path(
                    subpackage_path, str(manifest_path)
                )

            if not subpackage_path in all_manifest_paths:
                anonymous_subpackages.add(subpackage_path)
                all_manifest_paths.add(subpackage_path)

    if anonymous_subpackages:
        add_inputs_from_packages(
            anonymous_subpackages,
            all_manifest_paths,
            inputs,
            in_subpackage=True,
        )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate a hermetic inputs file that includes the outputs of Assembly"
    )
    parser.add_argument(
        "--partitions",
        type=argparse.FileType("r"),
        help="The partitions config that follows this schema: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/developer/ffx/plugins/assembly/#partitions-config",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="The location to write the hermetic inputs file",
    )
    parser.add_argument(
        "--system",
        nargs="*",
        help="A list of system image manifests that follow this schema: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/developer/ffx/plugins/assembly/#images-manifest",
    )
    parser.add_argument(
        "--depfile",
        help="A depfile listing all the files opened by this script",
    )
    args = parser.parse_args()

    # A list of files opened by this script.
    deps: set[FilePath] = set()

    # A list of the implicit inputs.
    inputs = []

    # Add all the bootloaders as inputs.
    if args.partitions:
        inputs.append(args.partitions.name)
        partitions = json.load(args.partitions)
        base_dir = os.path.dirname(args.partitions.name)
        for bootloader in partitions.get("bootloader_partitions", []):
            inputs.append(os.path.join(base_dir, bootloader["image"]))
        for bootstrap in partitions.get("bootstrap_partitions", []):
            inputs.append(os.path.join(base_dir, bootstrap["image"]))
        for credential in partitions.get("unlock_credentials", []):
            inputs.append(os.path.join(base_dir, credential))

    # Add all the system images as inputs.
    package_manifest_paths: set[FilePath] = set()
    for image_manifest_dir in args.system:
        image_manifest_path = image_manifest_dir + "/assembled_system.json"
        deps.add(image_manifest_path)
        with open(image_manifest_path, "r") as f:
            image_manifest = json.load(f)
            for image in image_manifest["images"]:
                inputs.append(os.path.join(image_manifest_dir, image["path"]))

                # Collect the package manifests from the blobfs/Fxblob image.
                if (image["type"] == "blk" and image["name"] == "blob") or (
                    image["type"] == "blk" and image["name"] == "fxfs.fastboot"
                ):
                    packages = []
                    packages.extend(
                        image["contents"]["packages"].get("base", [])
                    )
                    packages.extend(
                        image["contents"]["packages"].get("cache", [])
                    )
                    package_manifest_paths.update(
                        [
                            os.path.join(
                                image_manifest_dir, package["manifest"]
                            )
                            for package in packages
                        ]
                    )

            # Add all the bootloaders as inputs.
            if "partitions_config" in image_manifest:
                partitions_config_dir = (
                    image_manifest_dir
                    + "/"
                    + image_manifest["partitions_config"]
                )
                partitions_config_path = (
                    partitions_config_dir + "/partitions_config.json"
                )
                deps.add(partitions_config_path)
                with open(partitions_config_path, "r") as f:
                    partitions = json.load(f)
                    for bootloader in partitions.get(
                        "bootloader_partitions", []
                    ):
                        inputs.append(
                            os.path.join(
                                partitions_config_dir, bootloader["image"]
                            )
                        )
                    for bootstrap in partitions.get("bootstrap_partitions", []):
                        inputs.append(
                            os.path.join(
                                partitions_config_dir, bootstrap["image"]
                            )
                        )
                    for credential in partitions.get("unlock_credentials", []):
                        inputs.append(
                            os.path.join(partitions_config_dir, credential)
                        )

    # If we collected any package manifests, include all the blobs referenced
    # by them.
    all_manifest_paths: set[FilePath] = set(package_manifest_paths)
    add_inputs_from_packages(
        package_manifest_paths,
        all_manifest_paths,
        inputs,
    )
    deps.update(all_manifest_paths)

    # Write the hermetic inputs file.
    with open(args.output, "w") as f:
        f.writelines(f"{input}\n" for input in sorted(inputs))

    # Write the depfile.
    if args.depfile:
        with open(args.depfile, "w") as f:
            DepFile.from_deps(args.output, deps).write_to(f)

    return 0


if __name__ == "__main__":
    sys.exit(main())
