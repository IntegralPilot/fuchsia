#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates a content checklist file for inclusion in the IDK by reading the passed
package manifest, files (with specified dispositions), and reference to compare
with. For use in sdk_fuchsia_package() template."""

import argparse
import collections
import difflib
import json
import os
import subprocess
import sys
import tempfile


def get_meta_far_contents(ffx_bin, far_bin, meta_far_source_path):
    """
    Takes a path to a meta far file, and returns a list
    of tuples of form: (file_path, content_hash)
    """
    # Extract file paths from meta.far
    meta_far_paths_and_merkles = []
    meta_far_list_result = subprocess.run(
        [far_bin, "list", f"--archive={meta_far_source_path}"],
        stdout=subprocess.PIPE,
        text=True,
    )
    meta_far_file_paths = meta_far_list_result.stdout.split("\n")

    # tempdir used for isolating ffx
    with tempfile.TemporaryDirectory() as tmpdir:
        # Extract contents of file paths, and calculate content hash
        for file_path in sorted(meta_far_file_paths):
            if file_path == "":
                continue

            content = subprocess.run(
                [
                    ffx_bin,
                    "--isolate-dir",
                    tmpdir,
                    "package",
                    "far",
                    "cat",
                    meta_far_source_path,
                    file_path,
                ],
                stdout=subprocess.PIPE,
            )

            with tempfile.NamedTemporaryFile("wb") as temp_file:
                temp_file.write(content.stdout)
                temp_file.flush()
                file_hash = subprocess.run(
                    [
                        ffx_bin,
                        "--isolate-dir",
                        tmpdir,
                        "package",
                        "file-hash",
                        temp_file.name,
                    ],
                    stdout=subprocess.PIPE,
                    text=True,
                )

                file_content_hash = file_hash.stdout.split()[0]
                meta_far_paths_and_merkles.append(
                    (file_path, file_content_hash)
                )

    return meta_far_paths_and_merkles


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest", help="Path to the package manifest.", required=True
    )
    parser.add_argument(
        "--output",
        help="Path to the content checklist file to compute.",
        required=True,
    )
    parser.add_argument(
        "--ffx-bin", help="Path to ffx tooling.", required=False
    )
    parser.add_argument(
        "--far-bin", help="Path to far tooling.", required=False
    )
    parser.add_argument(
        "--expected-files-exact",
        action="append",
        help="Exact files to capture.",
        required=False,
        default=[],
    )
    parser.add_argument(
        "--expected-files-present",
        action="append",
        help="Present files to capture.",
        required=False,
        default=[],
    )
    parser.add_argument(
        "--reference",
        help="Path to the golden content checklist file",
        required=False,
    )
    parser.add_argument(
        "--warn",
        help="Whether content checklist changes should only cause warnings",
        action="store_true",
    )
    parser.add_argument(
        "--depfile", help="Path for generating depfile.", required=False
    )
    parser.add_argument(
        "--is-coverage",
        help="If yes, hash check will be downgraded to presence check to avoid hash differences from inclusion of debug data (see `//tools/cmc/build/cml.gni` for more).",
        action="store_true",
    )

    args = parser.parse_args()

    if not os.path.isfile(args.manifest):
        print(
            f"Manifest file not found at location '{args.manifest}'. Exiting.",
            file=sys.stderr,
        )
        return 1

    depfile_collection = {args.output: []}
    manifest = {}
    with open(args.manifest, "r") as manifest_file:
        manifest = json.load(manifest_file)

    paths_and_merkles = [
        (blob["path"], blob["merkle"]) for blob in manifest["blobs"]
    ]

    # Retrieve paths and merkles from `meta/` entities.
    if args.ffx_bin and args.far_bin:
        for blob in manifest["blobs"]:
            if blob["path"] == "meta/":
                meta_far_source_path = blob["source_path"]
                meta_far_paths_and_merkles = get_meta_far_contents(
                    args.ffx_bin, args.far_bin, meta_far_source_path
                )
                paths_and_merkles += meta_far_paths_and_merkles
                depfile_collection[args.output] += [meta_far_source_path]

                break

    generated_package_content_checklist = {}
    generated_package_content_checklist["version"] = manifest["version"]
    # OrderedDict used to keep files in sorted order, while keeping top-level
    # values in declared order.
    generated_package_content_checklist["content"] = {
        "files": collections.OrderedDict()
    }
    for path, merkle in paths_and_merkles:
        # Ensure no duplicate files.
        if path in generated_package_content_checklist["content"]["files"]:
            print(
                f"Path found multiple times in manifest: '{path}'",
                file=sys.stderr,
            )
            return 1

        if path in args.expected_files_present:
            # Add as a 'present' file.
            generated_package_content_checklist["content"]["files"][path] = {
                "present": True
            }
        if path in args.expected_files_exact:
            if args.is_coverage:
                # Add as a 'present' file.
                generated_package_content_checklist["content"]["files"][
                    path
                ] = {"present": True}
            else:
                # Add as an 'exact' file.
                generated_package_content_checklist["content"]["files"][
                    path
                ] = {"hash": merkle}

    # Ensure all expected files seen in manifest.
    expected_files_present = args.expected_files_present
    expected_files_exact = args.expected_files_exact
    if args.is_coverage:
        print(
            f"Warning: Hash checks downgraded to presence-checks. See 'is-coverage' flag in `//build/packages/generate_sdk_package_content_checklist.py` for more.",
            file=sys.stdout,
        )
        expected_files_present += args.expected_files_exact
        expected_files_exact = []

    for expected_file_present in expected_files_present:
        if (
            expected_file_present
            not in generated_package_content_checklist["content"][
                "files"
            ].keys()
        ):
            print(
                f"File declared 'present' not found manifest: '{expected_file_present}'. Files available from manifest and meta far are:",
                file=sys.stderr,
            )
            print(
                "\n".join(sorted([path for path, _ in paths_and_merkles])),
                file=sys.stderr,
            )
            return 1
    for expected_file_exact in expected_files_exact:
        if (
            expected_file_exact
            not in generated_package_content_checklist["content"][
                "files"
            ].keys()
        ):
            print(
                f"File declared 'exact' not found manifest: '{expected_file_exact}'. Files available from manifest and meta far are:",
                file=sys.stderr,
            )
            print(
                "\n".join(sorted([path for path, _ in paths_and_merkles])),
                file=sys.stderr,
            )
            return 1

    generated_package_content_checklist_str = json.dumps(
        generated_package_content_checklist, indent=2
    )

    with open(args.output, "w") as output_file:
        output_file.write(generated_package_content_checklist_str)

    # If present, ensure generated file matches golden.
    if args.reference is not None:
        # Absolute path used for more actionable error messages.
        reference_abs_path = os.path.abspath(args.reference)
        depfile_collection[args.output] += [args.reference]
        passed_golden = False
        if not os.path.isfile(args.reference):
            print(
                f"Golden file specified, but no file found at {reference_abs_path}.",
                file=sys.stderr,
            )
        else:
            with open(args.reference, "r") as manifest_file:
                golden = json.load(manifest_file)

            if args.is_coverage:
                # Must change golden to only use presence in case of debugdata.
                for path in golden["content"]["files"]:
                    golden["content"]["files"][path] = {"present": True}
            golden_str = json.dumps(golden, indent=2)

            if not generated_package_content_checklist_str == golden_str:
                print(
                    "Error: SDK package golden and generated content checklist file do not match.",
                    file=sys.stderr,
                )
                print(
                    "\n".join(
                        difflib.unified_diff(
                            golden_str.splitlines(),
                            generated_package_content_checklist_str.splitlines(),
                        )
                    ),
                    file=sys.stderr,
                )
            else:
                passed_golden = True

        if not passed_golden:
            print(
                "To overwrite the golden file location with the newly generated content checklist file, issue this command:",
                file=sys.stderr,
            )
            print(
                f'  mkdir -p "{os.path.dirname(reference_abs_path)}" && cp {os.path.abspath(args.output)} {reference_abs_path}',
                file=sys.stderr,
            )
            if not args.warn:
                return 1

    # Write out depfile
    if args.depfile and len(depfile_collection[args.output]) > 0:
        os.makedirs(os.path.dirname(args.depfile), exist_ok=True)
        with open(args.depfile, "w") as f:
            for out_file in sorted(depfile_collection.keys()):
                in_file_list = sorted(depfile_collection[out_file])
                f.write(f"{out_file}: {' '.join(in_file_list)}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
