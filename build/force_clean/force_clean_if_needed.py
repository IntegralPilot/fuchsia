#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Process build "force clean fences". If it detects that the build should be clobbered,
it will run `gn clean`.

A fence is crossed when a machine checks out a different revision and
there is a diff between $BUILD_DIR/.force_clean_fences and the content of the
build_fences.txt files or outputs of the get_fences.py script if they exist in
this repository or under vendor/*/build/force_clean/.
"""

# NOTE: Do not import pathlib, as this script must be kept as fast as possible.
import argparse
import os
import subprocess
import sys


def read_build_fences_file(path: str) -> str:
    """Read a build_fences.txt file and remove empty and comment lines from it."""
    result = ""
    for line in open(path):
        line = line.strip()
        if not line or line[0] == "#":
            continue
        result += line
        result += "\n"
    return result


def main():
    parser = argparse.ArgumentParser(
        description="Process build force-clean fences."
    )
    parser.add_argument(
        "--gn-bin",
        required=True,
        help="Path to prebuilt GN binary.",
    )
    parser.add_argument(
        "--checkout-dir",
        required=True,
        help="Path to $FUCHSIA_DIR.",
    )
    parser.add_argument(
        "--build-dir",
        required=True,
        help="Path to the root build dir, e.g. $FUCHSIA_DIR/out/default.",
    )
    parser.add_argument(
        "--output-status",
        help="Write a status to a file on exit. Will be 'clean: <msg>' if a clean was performed, or 'ok: <msg>' otherwise.",
    )
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    def _log(msg):
        if args.verbose:
            print(msg)

    # check that inputs are valid
    if not os.path.isfile(args.gn_bin):
        raise RuntimeError(f"{args.gn_bin} is not a file")
    if not os.path.isdir(args.checkout_dir):
        raise RuntimeError(f"{args.checkout_dir} is not a directory")
    if not os.path.isdir(args.build_dir):
        raise RuntimeError(f"{args.build_dir} is not a directory")

    # Find the main build_fences.txt file and get_fences.py script and all those under vendor/*/
    all_fences_scripts = []
    all_fences_files = []
    subdirs = [args.checkout_dir]
    vendor_dir = os.path.join(args.checkout_dir, "vendor")
    if os.path.exists(vendor_dir):
        subdirs.extend(os.listdir(vendor_dir))
    for subdir in subdirs:
        script_path = os.path.join(
            subdir, "build", "force_clean", "get_fences.py"
        )
        if os.path.exists(script_path):
            all_fences_scripts.append(script_path)
        file_path = os.path.join(
            subdir, "build", "force_clean", "build_fences.txt"
        )
        if os.path.exists(file_path):
            all_fences_files.append(file_path)

    current_fences = []

    # read fences file directly.
    for path in sorted(all_fences_files):
        current_fences += [read_build_fences_file(path)]

    # execute fences scripts using the same interpreter as us
    for script in sorted(all_fences_scripts):
        _log(f"generating clean-build fences from {script}")
        current_fences.append(
            subprocess.run(
                [sys.executable, "-S", script],
                stdout=subprocess.PIPE,
                text=True,
                check=True,
            ).stdout
        )

    current_fences = "\n".join(current_fences)

    existing_fences = None
    existing_fences_path = os.path.join(args.build_dir, ".force_clean_fences")
    if os.path.exists(existing_fences_path):
        _log("reading existing build dir's force-clean fences")
        with open(existing_fences_path, "r") as f:
            existing_fences = f.read()

    def _write_fences():
        _log(
            f"writing new fences:\n=============\n{current_fences}\n============="
        )
        with open(existing_fences_path, "w") as f:
            f.write(current_fences)

    # clobber if needed
    if existing_fences == None:
        _log("no fences file found, assuming nothing to clean")
        _write_fences()
        status = "ok: no fences found."

    elif existing_fences != current_fences:
        print(f"new //build/force_clean/ fences found, clobbering build...")
        subprocess.run([args.gn_bin, "clean", args.build_dir])
        status = "clean: new fences found."
        _write_fences()
    else:
        _log("force_clean fences up-to-date, not clobbering")
        status = "ok: fences are up-to-date."

    if args.output_status:
        os.makedirs(os.path.dirname(args.output_status), exist_ok=True)
        with open(args.output_status, "w") as f:
            f.write(status)

    return 0


if __name__ == "__main__":
    sys.exit(main())
