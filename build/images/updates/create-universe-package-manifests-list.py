#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys
from collections import OrderedDict


def read_manifest_list(
    maybe_manifest_list_file_path: str,
) -> OrderedDict[str, str]:
    manifest_list: OrderedDict[str, str] = OrderedDict()
    if maybe_manifest_list_file_path:
        with open(maybe_manifest_list_file_path) as manifest_list_file:
            contents = manifest_list_file.read()
            if not contents:
                # `manifest_list_file` can be an empty file...
                return manifest_list

            manifest_list_object = json.loads(contents)
            for manifest_path in manifest_list_object["content"]["manifests"]:
                with open(manifest_path) as f:
                    manifest = json.load(f)
                    manifest_list[manifest["package"]["name"]] = manifest_path

    return manifest_list


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Create a single list of packages that are only in the GN metadata-based "
        "'universe' package set, by stripping all assembly-contributed packages from "
        "those found by GN metadata walk."
    )
    parser.add_argument("--depfile", help="If provided, create this depfile")
    parser.add_argument(
        "-o",
        "--output",
        help="If provided, write to this output package manifest list rather than stdout",
    )
    parser.add_argument(
        "--assembly-base-manifests-list",
        help="base package manifest list from assembly",
    )
    parser.add_argument(
        "--assembly-cache-manifests-list",
        help="cache package manifest list from assembly",
    )
    parser.add_argument(
        "--assembly-ondemand-manifests-list",
        help="on_demand package manifest list from assembly",
    )
    parser.add_argument(
        "--metadata-walk-manifests-list",
        required=True,
        help="universe package manifest list from GN metadata walk",
    )
    args = parser.parse_args()

    base_list = read_manifest_list(args.assembly_base_manifests_list)
    cache_list = read_manifest_list(args.assembly_cache_manifests_list)
    ondemand_list = read_manifest_list(args.assembly_ondemand_manifests_list)
    metadata_walk_list = read_manifest_list(args.metadata_walk_manifests_list)

    # The following checks _shouldn't_ be necessary, as product assembly should
    # never be creating a situation where the same package is in multiple
    # sets.  They should probably be removed.
    failures = []

    # make sure the cache package list doesn't have any base packages.
    for name, path in cache_list.items():
        if name in base_list:
            failures.append(f"package {name} is both a base and cache package")

    for name, path in ondemand_list.items():
        if name in base_list:
            failures.append(
                f"package {name} is both a base and on_demand package"
            )
            sys.exit(1)

    for name, path in ondemand_list.items():
        if name in cache_list:
            failures.append(
                f"package {name} is both a cache and on_demand package"
            )

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        sys.exit(1)

    manifest_paths = []
    for name, path in metadata_walk_list.items():
        if name in base_list or name in cache_list or name in ondemand_list:
            continue

        manifest_paths.append(path)

    out_package_manifest_list = {
        "content": {"manifests": manifest_paths},
        "version": "1",
    }

    with open(args.output, "w") as f:
        json.dump(out_package_manifest_list, f, indent=2, sort_keys=True)

    if args.depfile:
        with open(args.depfile, "w") as f:
            deps = list(base_list.values())
            deps.extend(cache_list.values())
            deps.extend(ondemand_list.values())
            deps.extend(metadata_walk_list.values())

            f.write(f"{args.output}: {' '.join(deps)}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
