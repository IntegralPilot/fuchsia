#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generate a hermetic inputs file for image assembly by reading the image
assembly inputs and generating a list of all the files that are read"""

import argparse
import json
import os
import sys
from collections.abc import Iterable

from assembly import FilePath, ImageAssemblyConfig, PackageManifest
from depfile import DepFile
from serialization import json_load


def get_relative_path(relative_path: str, relative_to_file: str) -> str:
    file_parent = os.path.dirname(relative_to_file)
    path = os.path.join(file_parent, relative_path)
    path = os.path.realpath(path)
    path = os.path.relpath(path, os.getcwd())
    return path


def files_from_package_set(
    package_set: Iterable[FilePath], deps: set[FilePath]
) -> set[FilePath]:
    paths: set[FilePath] = set()
    for manifest_path in package_set:
        manifest = str(manifest_path)
        paths.add(manifest)
        with open(manifest, "r") as file:
            package_manifest = json_load(PackageManifest, file)
            blob_sources = []
            for blob in package_manifest.blobs:
                path = str(blob.source_path)
                if package_manifest.blob_sources_relative:
                    path = get_relative_path(path, manifest)
                blob_sources.append(path)
            paths.update(blob_sources)
            if package_manifest.subpackages:
                subpackage_set = []
                for subpackage in package_manifest.subpackages:
                    path = str(subpackage.manifest_path)
                    if package_manifest.blob_sources_relative:
                        path = get_relative_path(path, manifest)
                    if path not in deps:
                        subpackage_set.append(path)
                paths.update(subpackage_set)
                deps.update(subpackage_set)
                paths.update(files_from_package_set(subpackage_set, deps))
    return paths


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--image-assembly-config",
        required=True,
        help="The path to the image assembly config file",
    )
    parser.add_argument(
        "--output",
        type=str,
        required=True,
        help="The path to the first output of the image assembly target",
    )
    parser.add_argument(
        "--depfile",
        required=True,
        help="The path to the depfile for this script",
    )

    args = parser.parse_args()
    deps: set[FilePath] = set()
    inputs: set[FilePath] = set()

    with open(args.image_assembly_config, "r") as f:
        config = json_load(ImageAssemblyConfig, f)

        # Collect the list of files that are read in this script.
        deps.update(config.base)
        deps.update(config.cache)
        deps.update(config.system)
        deps.update(config.bootfs_packages)

        # Collect the list of inputs to image assembly.
        inputs.update(files_from_package_set(config.base, deps))
        inputs.update(files_from_package_set(config.cache, deps))
        inputs.update(files_from_package_set(config.system, deps))
        inputs.update(files_from_package_set(config.bootfs_packages, deps))
        inputs.update([entry.source for entry in config.bootfs_files])
        if config.kernel.path:
            inputs.add(config.kernel.path)
        if config.zbi_extra_items:
            inputs.add(config.zbi_extra_items)
        if config.devicetree:
            inputs.add(config.devicetree)
        if config.devicetree_overlay:
            inputs.add(config.devicetree_overlay)
        if config.qemu_kernel:
            inputs.add(config.qemu_kernel)

        # Add the files from the partitions config.
        if config.partitions_config:
            path = os.path.join(
                config.partitions_config, "partitions_config.json"
            )
            deps.add(path)
            inputs.add(path)
            with open(path, "r") as f:
                partitions_config = json.load(f)
                partitions = partitions_config.get("bootstrap_partitions", [])
                for part in partitions:
                    inputs.add(
                        os.path.join(config.partitions_config, part["image"])
                    )
                partitions = partitions_config.get("bootloader_partitions", [])
                for part in partitions:
                    inputs.add(
                        os.path.join(config.partitions_config, part["image"])
                    )
                creds = partitions_config.get("unlock_credentials", [])
                for cred in creds:
                    inputs.add(os.path.join(config.partitions_config, cred))

    # Add the files from the images config.
    with open(args.image_assembly_config, "r") as f:
        image_assembly_config = json.load(f)
        images_config = image_assembly_config["images_config"]
        for image in images_config["images"]:
            if image["type"] == "vbmeta":
                if "key" in image:
                    inputs.add(image["key"])
                if "key_metadata" in image:
                    inputs.add(image["key_metadata"])
                inputs.update(image.get("additional_descriptor_files", []))
            elif image["type"] == "zbi":
                if "postprocessing_script" in image:
                    script = image["postprocessing_script"]
                    if "path" in script and script["path"]:
                        inputs.add(script["path"])
                    if (
                        "board_script_path" in script
                        and script["board_script_path"]
                    ):
                        script_dir = os.path.dirname(
                            script["board_script_path"]
                        )
                        for root, _, files in os.walk(script_dir):
                            for file in files:
                                inputs.add(os.path.join(root, file))

    if deps:
        with open(args.depfile, "w") as depfile:
            DepFile.from_deps(args.output, deps).write_to(depfile)

    with open(args.output, "w") as f:
        for input in inputs:
            f.write(f"{input}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
