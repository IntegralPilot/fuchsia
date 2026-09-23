#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
from pathlib import Path
from typing import Any

# SDK directory of blobs across all package manifests,
# each renamed to their merkle.
BLOBS_DIR = "packages/blobs"

# SDK directory of subpackage manifests, each renamed to their merkle.
SUBPACKAGE_MANIFEST_DIR = "packages/subpackage_manifests"

# sdk://
# └── packages/
#     ├── blobs/
#     │   └── CONTENT_HASH_1
#     ├── PACKAGE_BAR/
#     │   ├── VARIANT/
#     │   │   └── release/
#     │   │       └── package_manifest.json
#     │   └── meta.json
#     └── subpackage_manifests/
#         └── META_FAR_MERKLE.package_manifest.json
# SDK directory of blobs, relative to each SDK package.
PACKAGE_DIR_TO_BLOBS_DIR = "../../../blobs"
# SDK directory of blobs, relative to `subpackage_manifests`.
SUBPACKAGE_MANIFESTS_DIR_TO_BLOBS_DIR = "../blobs"
# SDK directory of subpackage manifests, relative to individual SDK packages.
PACKAGE_DIR_TO_SUBPACKAGE_MANIFESTS_DIR = "../../../subpackage_manifests"


def handle_package_manifest(
    input_manifest_path: Path,
    sdk_file_map: dict[str, str],
    sdk_metadata: dict[str, Any],
    inputs: dict[str, str],
    # Parameters only used in recursive calls.
    visited_subpackages: set[Path] = set(),
    is_subpackage: bool = False,
) -> tuple[str, dict[str, dict[str, Any]]]:
    """
    For the given `input_manifest_path`, does the following:
    * Re-writes all source paths to be relative to SDK location,
      where relative location is determined by if it is a subpackage.
    * Adds additional atom files to `sdk_file_map`.
    * Adds files and base package manifest to `sdk_metadata`.

    Above is recursed across all subpackages.

    Args:
        input_manifest_path:    Path to package manifest to convert into
                                SDK-friendly format.
        sdk_file_map:           Dictionary containing <dst>=<src> entries, for
                                use in the `sdk_atom`'s `file_list`.
        sdk_metadata:           Object used to build a
                                `//build/sdk/meta/package.json` entry.
        inputs:                 Input dictionary containing `api_level`, `arch`,
                                and `distribution_name`. Used for constructing
                                end paths and other structures.
        visited_subpackages:    Set used for tracking levels of recursion in
                                subpackages.
        is_subpackage:          Boolean used to track if this iteration is
                                processing a subpackages. Used to determine if
                                `sdk_metadata` changes are required, as well as
                                pathing.
    Returns a tuple containing:
    * The path to the package manifest (not to be confused with the atom's
      meta.json file).
    * A dictionary mapping the manifest paths for the package and all its
      subpackages to their JSON contents. Includes the path above.
    """
    api_level, arch, distribution_name = (
        inputs["api_level"],
        inputs["arch"],
        inputs["distribution_name"],
    )

    subtype = f"{arch}-api-{api_level}"

    with open(input_manifest_path, "r") as manifest_file:
        input_manifest: dict[str, Any] = json.load(manifest_file)

    # Re-wire will be relative to package manifest location.
    input_manifest["blob_sources_relative"] = "file"

    # Subpackage manifests have different relative paths.
    relative_blobs_dir = (
        SUBPACKAGE_MANIFESTS_DIR_TO_BLOBS_DIR
        if is_subpackage
        else PACKAGE_DIR_TO_BLOBS_DIR
    )
    relative_subpackage_manifests_dir = (
        "" if is_subpackage else PACKAGE_DIR_TO_SUBPACKAGE_MANIFESTS_DIR
    )

    target_files = []
    # `meta_far_merkle` necessary for naming subpackage manifests.
    meta_far_merkle = ""
    for blob in input_manifest["blobs"]:
        merkle, source_path = blob["merkle"], blob["source_path"]
        relative_blob_path = f"{relative_blobs_dir}/{merkle}"

        if blob["path"] == "meta/":
            meta_far_merkle = merkle

        # Re-wire source path to point to SDK blob store.
        blob["source_path"] = relative_blob_path

        sdk_file_map[f"{BLOBS_DIR}/{merkle}"] = source_path
        target_files.append(f"{BLOBS_DIR}/{merkle}")

    # Handle subpackages.
    subpackages_manifests_info: dict[str, dict[str, Any]] = {}
    subpackage_list = input_manifest.get("subpackages", [])
    for subpackage in subpackage_list:
        subpackage_manifest_path, subpackage_merkle = (
            subpackage["manifest_path"],
            subpackage["merkle"],
        )
        # Re-wire subpackage manifest paths.
        subpackage[
            "manifest_path"
        ] = f"{relative_subpackage_manifests_dir}/{subpackage_merkle}"

        if subpackage_manifest_path in visited_subpackages:
            # No need to re-visit same subpackage multiple times.
            continue

        # Recursively handle subpackages.
        sdk_output_manifest_path, manifests_info = handle_package_manifest(
            subpackage_manifest_path,
            sdk_file_map,
            sdk_metadata,
            inputs,
            visited_subpackages,
            is_subpackage=True,
        )
        target_files.append(sdk_output_manifest_path)
        subpackages_manifests_info.update(manifests_info)

    if is_subpackage:
        manifest_file_name = f"{meta_far_merkle}.package_manifest.json"
        sdk_output_manifest_path = (
            f"{SUBPACKAGE_MANIFEST_DIR}/{manifest_file_name}"
        )

        visited_subpackages.add(input_manifest_path)
    else:
        manifest_file_name = "package_manifest.json"

        sdk_output_manifest_path = f"packages/{distribution_name}/{subtype}/release/{manifest_file_name}"

        target_files.append(sdk_output_manifest_path)
        target_files = sorted(list(set(target_files)))
        # Ensure metadata is aware of SDK manifest location.
        sdk_metadata["variants"] = [
            {
                "manifest_file": sdk_output_manifest_path,
                "arch": arch,
                "api_level": str(api_level),
                "files": target_files,
            }
        ]
        # Ensure package name matches distribution name.
        input_manifest["package"]["name"] = distribution_name

    # Add this package's manifest.
    subpackages_manifests_info[sdk_output_manifest_path] = input_manifest

    return sdk_output_manifest_path, subpackages_manifests_info
