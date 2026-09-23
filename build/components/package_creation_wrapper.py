#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Wraps the package creation tool with a script that converts the
GN-generated files into the inputs that it needs, in a single
python process.
"""


import argparse
import json
import subprocess
import sys
from pathlib import Path

import distribution_manifest
from depfile import DepFile


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--distribution-manifest",
        type=Path,
        help="GN-generated distribution manifest",
    )
    parser.add_argument(
        "--fini-manifest",
        type=Path,
        help="path to the package-creation fini manifest to write",
    )
    parser.add_argument(
        "--package-tool",
        type=Path,
        help="path to the package-tool executable",
    )
    parser.add_argument(
        "--output-dir", type=Path, help="directory to write the package to"
    )
    parser.add_argument(
        "--repository",
        help="hostname of the repository the package will be published to",
    )
    parser.add_argument(
        "--api-level",
        help="package api level",
    )
    parser.add_argument(
        "--depfile",
        type=Path,
        help="path to write a depfile to",
    )
    parser.add_argument(
        "--subpackages-manifest",
        type=Path,
        required=False,
        help="path to the manifest of subpackages this package has",
    )
    parser.add_argument(
        "--verify-elf-binaries",
        action="store_true",
        help="enable the verification of ELF binaries",
    )
    parser.add_argument(
        "--toolchain-lib-dir",
        default=[],
        action="append",
        metavar="DIR",
        help="Path to toolchain-provided lib directory. Can be used multiple times.",
    )
    parser.add_argument(
        "--validate-structured-config",
        type=Path,
        metavar="CONFIGC_PATH",
        help="path to the configc tool to use to validate structured config",
    )
    args = parser.parse_args()

    package_manifest = args.output_dir / "package_manifest.json"

    # Gather all the destination->source pairs from the distribution entry
    # data.
    inputs: set[str] = set()

    with open(args.distribution_manifest) as f:
        inputs.add(str(args.distribution_manifest))
        dist_manifest_json = json.load(f)
        entries, error = distribution_manifest.expand_manifest(
            dist_manifest_json, inputs
        )

    if error:
        print(error, file=sys.stderr)
        return -1

    # Write out the package creation manifest
    with open(args.fini_manifest, "w") as f:
        for entry in sorted(entries, key=lambda x: x.destination):
            f.write(f"{entry.destination}={entry.source}\n")

    # Optionally verify the ELF binaries in the package
    if args.verify_elf_binaries:
        # Only import the elf verification script when it's needed, to save
        # whatever few ms that it might give us.
        from elf.verify_manifest_elf_binaries import (
            VerificationFailure,
            verify_manifest_elf_binaries,
        )

        try:
            verification_inputs = verify_manifest_elf_binaries(
                {entry.destination: str(entry.source) for entry in entries},
                args.toolchain_lib_dir,
            )
            inputs.update(verification_inputs)
        except VerificationFailure as e:
            print(e, file=sys.stderr)
            return -2

    # Construct the call to the package building tool
    pkg_build_command: list[str] = [
        str(args.package_tool),
        "package",
        "build",
        str(args.fini_manifest),
        "-o",
        str(args.output_dir),
        "--repository",
        args.repository,
        "--api-level",
        args.api_level,
        "--depfile",
        "--blobs-json",
        "--blobs-manifest",
    ]

    # Add subpackages if present
    if args.subpackages_manifest:
        pkg_build_command += [
            "--subpackages-build-manifest-path",
            str(args.subpackages_manifest),
        ]

    # Run the package-tool
    proc = subprocess.run(pkg_build_command)
    if proc.returncode != 0:
        return proc.returncode

    # Validate the package's structured config, if requested to do so
    if args.validate_structured_config:
        configc_path: Path = args.validate_structured_config
        validator = subprocess.run(
            [
                str(configc_path),
                "validate-package",
                str(package_manifest),
                "--stamp",
                "/dev/null",
            ]
        )
        if validator.returncode != 0:
            print(
                """
Validating structured configuration failed!

If this is a fuchsia_test_package() and you are using
RealmBuilder to provide all values, consider setting
`validate_structured_config=false` on this target to
disable this check.
""",
                file=sys.stderr,
            )
            sys.exit(validator.returncode)

    # Read the depfile written by the the package-tool
    with open(args.output_dir / "meta.far.d") as file:
        pkg_tool_depfile = DepFile.read_from(file)
        inputs.update([str(s) for s in pkg_tool_depfile.deps])

    # Write it back out as our own, after adding our own inputs to it
    with open(args.depfile, "w") as depfile:
        DepFile.from_deps(package_manifest, inputs).write_to(depfile)

    return 0


if __name__ == "__main__":
    sys.exit(main())
