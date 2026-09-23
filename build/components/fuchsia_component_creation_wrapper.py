#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Wraps the component manifest creation process with a script that combines
each of the separate steps into a single python process.
"""


import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from depfile import DepFile


def check_expected_includes(
    expected_includes: Path,
    manifest_path_file: Path,
    cmc: Path,
    includeroot: Path,
    includepaths: list[Path],
    depfile: DepFile,
    expected_includes_depfile: Path,
) -> int:
    with open(expected_includes) as expected_includes_file:
        if expected_includes_file.read() == "":
            # nothing to expect, exit early
            return 0

    with open(manifest_path_file) as f:
        manifest_path_json = json.load(f)
        if len(manifest_path_json) != 1:
            print(
                "ERROR:  --manifest-path-file must be a json list containing a single file path",
                file=sys.stderr,
            )
            return -2
        manifest_path: str = manifest_path_json[0]

    expected_includes_cmd: list[str | Path] = [
        cmc,
        "--stamp",
        "/dev/null",
        "check-includes",
        manifest_path,
        "--fromfile",
        expected_includes,
        "--depfile",
        expected_includes_depfile,
        "--includeroot",
        includeroot,
    ]
    for p in includepaths:
        expected_includes_cmd += ["--includepath", p]
    expected_includes_result = subprocess.run(expected_includes_cmd)

    if expected_includes_result.returncode != 0:
        return expected_includes_result.returncode

    # Read the depfile written by cmc, and add its inputs to our own depfile's inputs
    # This depfile needs to be added to the action's implicit outputs, for action_tracer.py
    # to see
    depfile.add_output(expected_includes_depfile)
    with open(expected_includes_depfile) as file:
        depfile.update(DepFile.read_from(file).deps)

    return 0


def check_references(
    check_references_dist_manifest: Path,
    check_references_fini_manifest: Path,
    cmc: Path,
    compiled_manifest: Path,
    label: str,
    depfile: DepFile,
) -> int:
    import distribution_manifest

    # It needs to be recursively expanded and deduplicated in order to make
    # it into a list of files that
    depfile.add_input(str(check_references_dist_manifest))
    with open(check_references_dist_manifest) as f:
        dist_manifest_json = json.load(f)
        inputs: set[str] = set()
        entries, error = distribution_manifest.expand_manifest(
            dist_manifest_json, inputs
        )
        depfile.update(inputs)

    # Write out a package creation manifest for the references, to use with CMC
    # This file needs to be added to the action's implicit outputs, for action_tracer.py
    # to see
    depfile.add_output(check_references_fini_manifest)
    with open(check_references_fini_manifest, "w") as fini_manifest:
        for entry in sorted(entries, key=lambda x: x.destination):
            fini_manifest.write(f"{entry.destination}={entry.source}\n")

    check_references_result = subprocess.run(
        [
            cmc,
            "validate-references",
            "--component-manifest",
            compiled_manifest,
            # This is actually a "package creation manifest", or a "fini" manifest
            "--package-manifest",
            check_references_fini_manifest,
            "--context",
            label,
        ]
    )
    if check_references_result.returncode != 0:
        return check_references_result.returncode

    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--label", required=True, help="GN label of this action"
    )
    parser.add_argument(
        "--compiled-manifest",
        type=Path,
        required=True,
        help="Path of the compiled manifest being validated",
    )
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Destination path for the validated compiled manifest",
    )
    parser.add_argument(
        "--depfile", type=Path, help="Path to a depfile to write when done"
    )
    parser.add_argument(
        "--cmc",
        type=Path,
        help="Path to the 'cmc' tool",
    )
    parser.add_argument(
        "--manifest-path-file",
        type=Path,
        help="Path to a file that contains the path of the CML source file for the compiled manifest",
    )
    parser.add_argument(
        "--expected-includes",
        type=Path,
        help="Path to a file containing a list of cml files that are expected to be included by '--manifest-path-file'",
    )
    parser.add_argument(
        "--includeroot",
        help="Path to root of includes.",
        type=Path,
    )
    parser.add_argument(
        "--includepath",
        help="Additional path for relative includes.",
        type=Path,
        nargs="+",
    )
    parser.add_argument(
        "--check-references-dist-manifest",
        type=Path,
        help="Input distribution manifest used to check includes",
    )
    parser.add_argument(
        "--check-references-fini-manifest",
        type=Path,
        help="Intermediate fini manifest to produce when checking references",
    )
    args = parser.parse_args()

    # Some error-checking on provided arguments just to help make it a bit more clear when
    # something is mishandled.
    if (
        args.expected_includes
        or (
            args.check_references_dist_manifest
            or args.check_references_fini_manifest
        )
    ) and not args.cmc:
        if not args.cmc:
            print(
                "ERROR: --cmc must be provided with --expect-includes and --check-manifest-*",
                file=sys.stderr,
            )
            return -1

    if bool(args.check_references_dist_manifest) ^ bool(
        args.check_references_fini_manifest
    ):
        print(
            "ERROR: need both or neither of --check-references-dist-manifest and --check-references-fini-manifest",
            file=sys.stderr,
        )
        return -1

    # Setup a depfile for the action
    depfile = DepFile(args.output)

    # If requested, validate that the cml source file includes all the expected cml shards that
    # the binary's libraries require.
    if args.expected_includes:
        if args.manifest_path_file:
            rc = check_expected_includes(
                args.expected_includes,
                args.manifest_path_file,
                args.cmc,
                args.includeroot,
                args.includepath,
                depfile,
                args.output.parent / f"{args.output.name}.d",
            )
            if rc != 0:
                return rc
        else:
            print(
                "ERROR: --manifest-path-file must be provided with --expected-includes",
                file=sys.stderr,
            )
            return -1

    # If requested, check that the files reference by the compiled component are all
    # included as distribution entries of this component.
    if args.check_references_dist_manifest:
        rc = check_references(
            args.check_references_dist_manifest,
            args.check_references_fini_manifest,
            args.cmc,
            args.compiled_manifest,
            args.label,
            depfile,
        )
        if rc != 0:
            return rc

    # After validation, copy the input manifest to the output location to signify completion
    if args.output.exists():
        args.output.unlink()
    shutil.copy(args.compiled_manifest, args.output)
    depfile.add_input(args.compiled_manifest)

    # And write out the depfile
    with open(args.depfile, "w") as f:
        depfile.write_to(f)

    return 0


if __name__ == "__main__":
    sys.exit(main())
