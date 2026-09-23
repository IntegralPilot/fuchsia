#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys
from pathlib import Path

import bazel_compdb_utils
import build_utils

# Set this to True to debug operations locally in this script.
_DEBUG = False


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Merge bazel compdb from all bazel_actions based on the input build API module",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        required=True,
        help="The build directory",
    )
    parser.add_argument(
        "--compdb",
        type=Path,
        required=True,
        help="The compdb file to merge Bazel compdb into",
    )
    parser.add_argument(
        "--bazel-build-action-targets",
        type=Path,
        required=True,
        help="The bazel build action targets file",
    )

    args = parser.parse_args()

    time_profile = build_utils.TimeProfile()

    time_profile.start("load_compdb", "Loading {}".format(args.compdb))

    with open(args.compdb, "r") as f:
        compdb = json.load(f)

    time_profile.start(
        "load_bazel_build_action_targets",
        "Loading {}".format(args.bazel_build_action_targets),
    )

    with open(args.bazel_build_action_targets, "r") as f:
        bazel_build_action_targets = json.load(f)

    time_profile.start("merge_compdb", "Merging compdb")

    for entry in bazel_build_action_targets:
        if "bazel_compdb_file" not in entry:
            continue

        compdb_fragment_path = args.build_dir / entry["bazel_compdb_file"]
        if _DEBUG:
            print(
                "Merging {}".format(compdb_fragment_path),
                file=sys.stderr,
            )

        # There is no guarantee that all targets from bazel_build_action_targets are built when this
        # script is run.
        if not compdb_fragment_path.exists():
            if _DEBUG:
                print(
                    "Compdb fragment {} does not exist".format(
                        compdb_fragment_path
                    ),
                    file=sys.stderr,
                )
            continue

        with open(compdb_fragment_path, "r") as f:
            bazel_compdb = json.load(f)
            compdb.extend(bazel_compdb)

    time_profile.start("write_compdb", "Writing {}".format(args.compdb))
    with open(args.compdb, "w") as f:
        json.dump(bazel_compdb_utils.dedupe(compdb), f, indent=2)

    time_profile.stop()

    if _DEBUG:
        time_profile.print(0.001)

    return 0


if __name__ == "__main__":
    sys.exit(main())
