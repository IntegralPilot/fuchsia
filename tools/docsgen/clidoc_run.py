#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shutil
import subprocess
import sys
from os import path


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Runs clidoc for the host tools in the IDK manifest.",
    )
    parser.add_argument(
        "--clidoc",
        type=str,
        required=True,
        help="clidoc executable",
    )
    parser.add_argument(
        "--input",
        type=str,
        required=True,
        help="SDK molecule manifest",
    )
    parser.add_argument(
        "--output",
        type=str,
        required=True,
        help="Output location where the contents should be listed.",
    )
    parser.add_argument(
        "--depfile",
        type=str,
        required=True,
        help="Depfile location.",
    )
    parser.add_argument(
        "--isolate-dir",
        type=str,
        required=True,
    )
    parser.add_argument(
        "--subtool-manifest",
        type=str,
        required=True,
    )
    parser.add_argument("--excludes", type=str, nargs="*")

    args = parser.parse_args()

    if not os.path.exists(args.input):
        raise ValueError(f"{args.input} does not exist.")

    contents, meta_files_read = read_sdk_molecule(args.input)

    run_clidoc(args, contents)

    # Clidoc creates a new depfile, so append the meta files after it has run.
    with open(args.depfile, "a") as dep_file:
        for f in meta_files_read:
            dep_file.write(f"{args.output}: {f}\n")

    return 0


def run_clidoc(args: argparse.Namespace, cmd_list: list[str]) -> None:
    outdir = path.join(args.isolate_dir, "docs")
    if path.exists(outdir):
        shutil.rmtree(outdir)
    os.makedirs(outdir)

    # The `split` gets just the command without the path for checking against
    # the exclude list.
    cmds = [c for c in cmd_list if c.split("/")[-1] not in args.excludes]

    idk_base_dir = os.path.dirname(os.path.dirname(args.input))

    clidoc_args = [
        args.clidoc,
        "--quiet",
        "--in-dir",
        idk_base_dir,
        "--out-dir",
        outdir,
        "--archive-path",
        args.output,
        "--depfile",
        args.depfile,
        "--subtool-manifest",
        args.subtool_manifest,
    ] + cmds
    subprocess.run(clidoc_args)


def read_sdk_molecule(manifest: str) -> tuple[list[str], list[str]]:
    with open(manifest) as f:
        data = json.load(f)

    idk_base_dir = os.path.dirname(os.path.dirname(manifest))

    cmds = []
    meta_files_read = []
    for part in data["parts"]:
        if part["type"] == "host_tool":
            meta_path = os.path.join(idk_base_dir, part["meta"])
            meta_files_read.append(meta_path)
            with open(meta_path) as meta_f:
                meta_data = json.load(meta_f)
                for f in meta_data["files"]:
                    if not f.endswith(".json"):
                        cmds.append(f)
    return cmds, meta_files_read


if __name__ == "__main__":
    sys.exit(main())
