#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Script to extract line-separated cache package paths with image_assembly.json from `ffx assembly product`.
"""

import argparse
import json
import os
import sys
from pathlib import Path


def _rebase_bazel_path(path: str, rebase_prefix: str) -> str:
    """Rebase paths relative to the Bazel execroot if needed.

    This also takes care of paths that belong to the @gn_targets
    repository. Because the content of this repository is regenerated
    on every bazel_action() command launched from Ninja, these paths
    may be dangling symlinks. However, they are intentionally formatted
    in a way that allows extracting the corresponding Ninja path.

    These can appear in two ways:

    - Directly as a |path| value that starts with `external/gn_targets/`, e.g.:

        external/gn_targets/src/foo/_files/obj/src/foo/cmd

    - As a Bazel path to a symlink that points to an absolute path within
      the Bazel {OUTPUT_BASE}/external/gn_targets/ repository, e.g.:

        bazel-out/k8-fastbuild/bin/src/foo/cmd
         --symlink-->
           /work/out/default/gen/build/bazel/output_base/external/gn_targets/src/foo/_files/obj/src/foo/cmd

    In both cases, keeping the path components that follow the `/_files/` one gets the
    correct result (relative to the Ninja build dir, i.e. the current directory).

    Args:
      path: Input package path, relative to rebase_prefix.
      rebase_prefix: Either an empty string, or a path prefix to the output base
         that must contain a trailing separator.

    Returns:
      new path, relative to the current directory.
    """
    rebased_path = rebase_prefix + path
    if os.path.islink(rebased_path):
        path = os.readlink(rebased_path)

    # LINT.IfChange(gn_targets_dir)
    _, external_gn_targets, suffix = path.partition("external/gn_targets+/")
    # LINT.ThenChange(//build/bazel/toplevel.MODULE.bazel:gn_targets_dir)
    if external_gn_targets:
        _, files, ninja_path = suffix.partition("/_files/")
        assert files, f"Unexpected @gn_targets path: {path}"
        return ninja_path

    return rebased_path


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Parse assembly product output manifest to get packages for a particular package set."
    )
    parser.add_argument(
        "--assembly-manifest",
        type=argparse.FileType("r"),
        required=True,
        help="Path to image_assembly.json created by `ffx assembly product`.",
    )
    parser.add_argument(
        "--package-set",
        required=True,
        help="Package set to get the packages for",
    )
    parser.add_argument(
        "--bazel-execroot",
        required=False,
        help="Optionally rebase the package manifest paths relative to the Bazel execroot, from the assembly manifest.",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to which to write desired output list.",
    )
    args = parser.parse_args()

    assert not args.bazel_execroot or os.path.isdir(
        args.bazel_execroot
    ), "--bazel-execroot needs to specify a valid directory path!"
    rebase_prefix = (
        args.bazel_execroot.rstrip("/") + "/" if args.bazel_execroot else ""
    )

    assembly_manifest = json.load(args.assembly_manifest)
    package_manifests = [
        (
            # Normalize the rebased path to remove any internal ../..'s.
            os.path.normpath(_rebase_bazel_path(package, rebase_prefix))
        )
        for package in assembly_manifest[args.package_set]
    ]
    Path(args.output).write_text(
        json.dumps(
            {
                "content": {"manifests": package_manifests},
                "version": "1",
            },
            indent=2,
        )
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
