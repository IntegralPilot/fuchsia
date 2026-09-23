# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Build
### List which packages are built.

import argparse
import json
import os
import re
from collections.abc import Callable, Iterable, Iterator
from typing import Any


# Print all the packages in sorted order, one per line.
def print_packages(packages: Iterable[str]) -> None:
    for p in sorted(packages):
        print(p)


# Extracts the list of package names that are accepted by filter_ from a
# decoded package list manifest.
def extract_packages_from_listing(
    manifest_data: dict[str, Any],
    filter_: Callable[[str], bool],
    build_dir: str,
) -> Iterator[str]:
    packages: list[str] = []
    for manifest in manifest_data["content"]["manifests"]:
        manifest_path = os.path.join(build_dir, manifest)
        with open(manifest_path) as f:
            packages.append(json.load(f)["package"]["name"])
    return filter(filter_, packages)


def get_filter(
    pattern: str | None, exact: bool = False
) -> Callable[[str], bool]:
    if not pattern:
        return lambda s: True
    regex = re.compile(pattern)
    if exact:
        return lambda s: bool(regex.fullmatch(s))
    return lambda s: bool(regex.search(s))


def get_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="""
list-packages lists the packages that the build is aware of. These are
packages that can be rebuilt and/or pushed to a device.
Note: list-packages DOES NOT list all packages that *could* be built, only
those that are included in the current build configuration.
""",
        epilog="""
The package list is derived from all_package_manifests.list (which is produced
during image assembly / full build).
See https://fuchsia.dev/fuchsia-src/development/build/software_assembly/build_configuration
for more information about using these package sets.
""",
    )
    parser.add_argument(
        "-e",
        "--exact",
        action="store_true",
        help="match the pattern exactly against the full package name (regex fullmatch)",
    )
    parser.add_argument(
        "pattern",
        nargs="?",
        help="list only packages matching this regular expression (matches substrings by default, or full names with -e/--exact)",
    )
    return parser


def main() -> None:
    parser = get_parser()
    args = parser.parse_args()

    # If a custom regex for package names is provided, use that to filter
    # results; otherwise, return all results
    try:
        filter_ = get_filter(args.pattern, exact=args.exact)
    except re.error as e:
        parser.error(f"invalid regular expression '{args.pattern}': {e}")

    build_dir = os.environ.get("FUCHSIA_BUILD_DIR")
    if not build_dir:
        raise RuntimeError(
            'Environment variable "FUCHSIA_BUILD_DIR" is not set.'
        )

    manifest_list_path = os.path.join(build_dir, "all_package_manifests.list")
    if not os.path.exists(manifest_list_path):
        raise RuntimeError(
            f"'{manifest_list_path}' not found. Run 'fx build' or 'fx build updates' to assemble package manifests."
        )

    with open(manifest_list_path) as f:
        packages = extract_packages_from_listing(
            json.load(f), filter_, build_dir=build_dir
        )
    print_packages(packages)


if __name__ == "__main__":
    main()
