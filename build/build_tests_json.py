#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Generate tests.json.
"""

import json
import os
import pprint
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "bazel/scripts"))
from build_utils import CommandRunner


def build_tests_json(
    build_dir: Path,
    with_bazel_tests: bool = False,
    command_runner: CommandRunner | None = None,
    quiet: bool = True,
) -> set[Path]:
    """Generate the tests.json file.

    tests.json is created by merging two things:

    1) tests_from_metadata.json
       A collection of test specs found from a GN metadata walk. These test
       specs are copied without modification into the final tests.json.

    2) product_bundle_test_groups.json
       A file that declares a mapping of product bundle name to a specific set
       of tests found in another tests.json. These test specs will be modified
       to include `product_bundle: <name>` before merging it into the final
       tests.json.

    Args:
        build_dir: Fuchsia build directory.
        with_bazel_tests: Whether to export Bazel tests.
        command_runner: Optional command runner to use for running bazel commands.
        quiet: Whether to print status updates.

    Returns:
        A set of Path values for the input files read by this function.
    """
    tests_from_metadata_path = os.path.join(
        build_dir, "tests_from_metadata.json"
    )
    test_groups_path = os.path.join(
        build_dir, "obj", "tests", "product_bundle_test_groups.json"
    )
    tests_json_path = os.path.join(build_dir, "tests.json")
    product_bundles_json_path = os.path.join(build_dir, "product_bundles.json")

    # Open the list of tests that were collected from a GN metadata walk.
    with open(tests_from_metadata_path, "r") as f:
        tests = json.load(f)

    # Open the mapping between test sets and product bundle name.
    with open(test_groups_path, "r") as f:
        test_groups = json.load(f)

    # Open the list of product bundles that were collected from a GN metadata
    # walk.
    with open(product_bundles_json_path, "r") as f:
        product_bundles = json.load(f)
        product_bundle_names = [pb["name"] for pb in product_bundles]

    # For every group of tests that are supposed to target a specific product
    # bundle, we parse the tests, add `product_bundle: <name>` and add the test
    # to `tests`. When infra reads the final tests.json file, it will read that
    # field and know to flash the product bundle with <name> before running the
    # test.
    #
    # We also assert that the product bundle name is found in
    # product_bundles.json.
    for test_group in test_groups:
        product_bundle_name = test_group["product_bundle_name"]
        if product_bundle_name not in product_bundle_names:
            print(
                f"ERROR: {product_bundle_name} is not a valid product_bundle_name."
            )
            print("Available names are:")
            pprint.pp(product_bundle_names)
            sys.exit(1)

        environments = test_group.get("environments", [])
        product_bundle_tests_file = build_dir / test_group["tests_json"]
        # Read the tests.json that is assigned to this specific product bundle.
        with open(product_bundle_tests_file, "r") as inner_tests_json:
            product_bundle_tests = json.load(inner_tests_json)
        # Update the test spec to include the product bundle target and
        # environments.
        for test in product_bundle_tests:
            name = test["test"]["name"] + "-" + product_bundle_name
            test["test"]["name"] = name
            test["product_bundle"] = product_bundle_name
            if environments:
                test["environments"] = environments
        tests += product_bundle_tests

    if with_bazel_tests:
        # Now get the list of all Bazel tests.
        # TODO(digit): This is just host tests for now, also get the list of
        # Fuchsia Bazel tests.
        fuchsia_dir = Path(__file__).parent.parent
        assert (
            fuchsia_dir / ".jiri_manifest"
        ).exists(), f"Invalid Fuchsia source directory: {fuchsia_dir}"

        sys.path.insert(0, str(fuchsia_dir / "build/bazel/scripts"))
        import bazel_tests_utils
        import build_utils

        # NOTE: Do not use fuchsia_dir here, since unit tests run with a different source dir,
        # that BazelPaths.new() will find by walking up from build_dir.
        bazel_paths = build_utils.BazelPaths.new(build_dir=build_dir)
        bazel_tests_json, bazel_inputs = bazel_tests_utils.generate_tests_json(
            bazel_paths,
            command_runner,
            quiet=quiet,
        )
        tests += bazel_tests_json
    else:
        bazel_inputs = set()

    # Write the final list of tests to tests.json if the contents changed.
    contents_changed = True
    if Path(tests_json_path).exists():
        with open(tests_json_path, "r") as f:
            previous_tests = json.load(f)
            if previous_tests == tests:
                contents_changed = False
    if contents_changed:
        with open(tests_json_path, "w") as f:
            json.dump(tests, f, indent=2)

    return {
        Path(tests_from_metadata_path),
        Path(test_groups_path),
    } | bazel_inputs
