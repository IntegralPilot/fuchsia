#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
This script helps prepare a fake build directory needed for fx publish tests.
The fake build directory serves as the CWD and root build directory for the
orchestrated `fx publish` test invocation.
"""

import argparse
import filecmp
import json
import os
import shutil
import sys
from pathlib import Path

INPUTS = []
OUTPUTS = []


def copy_build_output(output_dir: Path, build_output: Path) -> None:
    # Resolve the src file.
    build_dir = Path(".").resolve()
    src = build_output.resolve().relative_to(build_dir)
    assert src.exists(), f"{build_output} does not exist!"

    # Prepare dest parent directory.
    dest = output_dir / build_output
    dest.parent.mkdir(parents=True, exist_ok=True)

    INPUTS.append(src)
    if not dest.is_file() or not filecmp.cmp(src, dest):
        OUTPUTS.append(dest)

        # Purge dest.
        if dest.is_symlink() or dest.is_file():
            dest.unlink()
        elif dest.exists():
            shutil.rmtree(dest)

        # Perform copy.
        if dest.is_dir():
            shutil.copytree(src, dest)
        else:
            shutil.copy(src, dest)
    else:
        # Keep action_tracer happy since we did a file comparison.
        INPUTS.append(dest)


def copy_packages(working_dir: Path) -> None:
    # If we symlink our package manifests, the `package-tool` run by
    # `publish.py` will follow the symlink first. This will not work since the
    # multiple `bazel build` invocations throughout a large infra build will
    # invalidate/wipe any paths/files within the bazel output_base.
    #
    # Thus, we need to copy instead of symlinking. Luckily CAS stores blob
    # references and dedupes, reducing the expense of copying.
    #
    # Note that the non-test case does not run into issues with resolving
    # bazel's symlink farm since a parent directory of the blobs/manifests are
    # symlinks, rather than the blobs/manifests themselves.
    def add_package(manifest_path: Path) -> None:
        copy_build_output(working_dir, manifest_path)
        manifest = json.loads(manifest_path.read_text())
        assert (
            manifest.get("blob_sources_relative", None) == "file"
        ), f"Expected the package manifest {manifest_path} to have file relative blob sources!"

        for blob in manifest["blobs"]:
            blob = Path(manifest_path.parent / blob["source_path"])
            copy_build_output(working_dir, blob)

        for subpackage in manifest.get("subpackages", []):
            add_package(
                Path(manifest_path.parent / subpackage["manifest_path"])
            )

    # Copy the transitive closure of each package.
    assembly_cache_packages = working_dir / "assembly_cache_packages.list"
    INPUTS.append(assembly_cache_packages)
    for package_manifest in json.loads(assembly_cache_packages.read_text())[
        "content"
    ]["manifests"]:
        add_package(Path(package_manifest))


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Copies assembly_cache_packages.list with all transitively referenced files, and package-tool."
    )
    parser.add_argument(
        "--working-dir",
        required=True,
        help="The fx publish working directory to populate. Expects `{args.working_dir}/assembly_cache_packages.list` to already exist.",
    )
    parser.add_argument(
        "--stamp-file",
        required=True,
        help="The file that's written to signal that the operation can been completed.",
    )
    parser.add_argument(
        "--depfile",
        help="If specified, write a depfile of the files that were touched.",
    )
    args = parser.parse_args()

    working_dir = Path(args.working_dir)

    copy_packages(working_dir)

    copy_build_output(working_dir, Path("host-tools") / "package-tool")

    if args.depfile:
        depfile = Path(args.depfile)
        depfile.parent.mkdir(parents=True, exist_ok=True)
        depfile.write_text(
            f'{" ".join([str(file) for file in OUTPUTS])}: {" ".join([str(file) for file in INPUTS])}'
        )

    with open(args.stamp_file, "w") as stamp_file:
        stamp_file.write("done")

    # Explicitly update the mtime of the output directory to avoid ninja convergence no-op flakes.
    # See https://fxbug.dev/361451367.
    os.utime(working_dir)

    return 0


if __name__ == "__main__":
    sys.exit(main())
