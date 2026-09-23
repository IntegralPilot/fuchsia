# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates a budgets configuration file for size checker tool.

This tool converts the old size budget file to the new format as part of RFC-0144 migration plan.

Usage example:
  python3 build/assembly/scripts/convert_size_limits.py \
    --size_limits out/default/size_checker.json \
    --product_config out/default/obj/build/images/fuchsia/fuchsia_product_config.json \
    --output out/default/size_budgets.json


The budget configuration is a JSON file used by `ffx assembly size-check` that contains two types
of budgets:
resource_budgets: applies to all the files matched by path across all package sets. It is
  used to allocate space for common resources, widely used. Blobs corresponding to the matched
  files are no longer charged to any packages. It has the following fields:
    name: string, human readable name for this budget.
    budget_bytes: integer, number of bytes the matched files have to fit in.
    creep_budget_bytes: integer, maximum number of bytes added by a given code change.
    paths: list of strings, file name to be matched in the package manifest.

package_set_budgets: size limit that applies to one or more packages. It has the following fields:
    name, budget_bytes, creep_budget_bytes: ditto.
    packages: list of string, path to package manifests that belongs to this set.
    merge: boolean, when true the packages blobs are deduplicate by hash before accounting.
      This affects the shared blobs accounting, and is intended to approximate merged packages
      such as base_package_manifest.json.
"""
import argparse
import collections
import json
import sys
from typing import Any


class InvalidInputError(Exception):
    pass


def convert_budget_format_by_pkg_names(
    component: dict[str, Any], all_manifests: dict[str, str]
) -> dict[str, Any]:
    """Converts a component budget to the new budget format.

    Args:
      component: dictionary, size checker configuration entry that specifies
      packages by name.
      all_manifests: dict[string, Path)], list of tuples of package name and manifest
      path.
    Returns:
      dictionary, new configuration with a name, a maximum size and the list of
      packages manifest to fit in the budget.
    """
    packages = sorted(
        [
            all_manifests[pkg_name]
            for pkg_name in set(component["pkgs"])
            if pkg_name in all_manifests
        ]
    )
    result = dict(
        name=component["component"],
        budget_bytes=component["limit"],
        creep_budget_bytes=component["creep_limit"],
        merge=False,
        packages=packages,
    )
    return result


def count_packages(
    budgets: list[dict[str, Any]], all_manifests: list[str]
) -> tuple[list[str], list[str]]:
    """Returns packages that are missing, or present in multiple budgets."""
    package_count = collections.Counter(
        package for budget in budgets for package in budget["packages"]
    )
    more_than_once = [
        package for package, count in package_count.most_common() if count > 1
    ]
    zero = [package for package in all_manifests if package_count[package] == 0]
    return more_than_once, zero


def make_package_set_budgets(
    size_limits: dict[str, Any], product_config: dict[str, Any]
) -> list[dict[str, Any]]:
    # Convert each budget to the new format and packages from base and cache.
    # Package from system belongs the system budget.
    all_manifests: list[str] = product_config.get(
        "base", []
    ) + product_config.get("cache", [])

    # All package manifest paths are either in the format of:
    #   some/path/to/<package_name>
    # or
    #   some/path/to/<package_name>/package_manifest.json
    #
    # So a map of <package_name> to path can be readily computed.
    manifests_by_name: dict[str, str] = {}
    for manifest_path in all_manifests:
        segments = manifest_path.split("/")
        if (
            segments[-1] == "package_manifest.json"
            or segments[-1] == "rebased_package_manifest.json"
        ):
            # packages that have gone through the archive and expansion, such as
            # for use with bazel assembly may have an '_expanded' suffix that
            # should be removed.
            name = segments[-2].removesuffix("_expanded")
            # Remove suffix added to all Bazel-built packages
            name = name.removesuffix("_fuchsia_package_pkg")
        else:
            name = segments[-1]
        manifests_by_name[name] = manifest_path

    components = size_limits.get("components", [])
    packages_budgets = [
        convert_budget_format_by_pkg_names(component, manifests_by_name)
        for component in components
    ]

    # Verify packages are in exactly one budget.
    more_than_once, zero = count_packages(packages_budgets, all_manifests)

    # Create a budget for the packages not matched by any other budget.
    if "core_limit" in size_limits:
        packages_budgets.append(
            dict(
                name="Core system+services",
                budget_bytes=size_limits["core_limit"],
                creep_budget_bytes=size_limits["core_creep_limit"],
                merge=False,
                packages=sorted(zero),
            )
        )

    if more_than_once:
        print("ERROR: Package(s) matched by more than one size budget:")
        for package in more_than_once:
            print(f" - {package}")
        raise InvalidInputError()  # Exit with an error code.

    # Assign all system packages from the product configuration to the
    # system package set.
    system_budget = next(
        (
            budget
            for budget in packages_budgets
            if budget["name"] == "/system (drivers and early boot)"
        ),
        None,
    )
    if system_budget:
        # De-duplicate blobs by hash accors the packages of this budget in order to
        # approximate the package merging that happens with base_package_manifest.
        system_budget["merge"] = True
        system_budget["packages"] = sorted(product_config.get("system", []))

    return sorted(packages_budgets, key=lambda budget: budget["name"])


def make_resources_budgets(
    size_limits: dict[str, str | list[str]]
) -> list[dict[str, Any]]:
    budgets = []
    if "distributed_shlibs" in size_limits:
        budgets.append(
            dict(
                name="Distributed shared libraries",
                paths=sorted(size_limits["distributed_shlibs"]),
                budget_bytes=size_limits["distributed_shlibs_limit"],
                creep_budget_bytes=size_limits[
                    "distributed_shlibs_creep_limit"
                ],
            )
        )
    if "icu_data" in size_limits:
        budgets.append(
            dict(
                name="ICU Data",
                paths=sorted(list(size_limits["icu_data"])),
                budget_bytes=size_limits["icu_data_limit"],
                creep_budget_bytes=size_limits["icu_data_creep_limit"],
            )
        )
    return budgets


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Converts the former size_checker.go budget file to the new format as part of RFC-0144"
    )
    parser.add_argument(
        "--size-limits", type=argparse.FileType("r"), required=True
    )
    parser.add_argument(
        "--image-assembly-config", type=argparse.FileType("r"), required=True
    )
    parser.add_argument("--output", type=argparse.FileType("w"), required=True)
    parser.add_argument("-v", "--verbose", action="store_true")
    parser.add_argument("--max-blob-contents-size", type=int, required=True)
    args = parser.parse_args()

    # Read all input configurations.
    size_limits = json.load(args.size_limits)
    image_assembly_config = json.load(args.image_assembly_config)

    # Format the resulting configuration file.
    try:
        # Convert all the budget formats.
        resource_budgets = make_resources_budgets(size_limits)
        package_set_budgets = make_package_set_budgets(
            size_limits, image_assembly_config
        )

        output_contents = dict(
            resource_budgets=resource_budgets,
            package_set_budgets=package_set_budgets,
            total_budget_bytes=args.max_blob_contents_size,
        )

        if "dedup_blobs" in size_limits:
            output_contents["dedup_blobs"] = size_limits["dedup_blobs"]

        # Ensure the outputfile is closed early.
        with args.output as output:
            json.dump(output_contents, output, indent=2)

        return 0
    except InvalidInputError:
        return 1


if __name__ == "__main__":
    sys.exit(main())
