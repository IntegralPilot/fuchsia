#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import shutil
import sys
from dataclasses import dataclass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifacts_json",
        required=True,
        help="The path to the list of artifacts to include in the bundle.",
    )
    parser.add_argument(
        "--path_prefix_to_strip",
        default="frozen-ctf-artifacts-subbuild",
        help="The prefix to strip from the input paths to the output.",
    )
    parser.add_argument(
        "--output_manifest",
        required=True,
        help="The path to output a new manifest with adjusted paths.",
    )
    parser.add_argument(
        "--depfile",
        required=True,
        help="The path to write a GN depfile.",
    )
    args = parser.parse_args()

    artifacts: list[str] = []
    with open(args.artifacts_json) as f:
        artifacts = json.load(f)

    output_directory = os.path.dirname(args.output_manifest)
    os.makedirs(
        os.path.join(output_directory, "cts", "host_x64"), exist_ok=True
    )

    @dataclass
    class CopiedFile:
        src: str
        dest: str

    output_artifacts: list[str] = []
    copied_files: list[CopiedFile] = []

    for path in artifacts:
        stripped = path.removeprefix(args.path_prefix_to_strip + "/")
        dest = os.path.join(output_directory, stripped)
        shutil.copyfile(path, dest)
        shutil.copymode(path, dest)  # Preserve executable bits
        copied_files.append(CopiedFile(path, dest))
        output_artifacts.append(stripped)

    with open(args.output_manifest, "w") as out_f:
        json.dump(output_artifacts, out_f, indent=2)

    with open(args.depfile, "w") as out_depfile:
        artifacts_inputs = " ".join(artifacts)

        # Output manifest depends on all inputs.
        out_depfile.writelines(
            [f"{args.output_manifest}: {artifacts_inputs}\n"]
        )

        # Each copied archive depends on its input.
        for copied_file in copied_files:
            out_depfile.writelines([f"{copied_file.dest}: {copied_file.src}\n"])

    return 0


if __name__ == "__main__":
    sys.exit(main())
