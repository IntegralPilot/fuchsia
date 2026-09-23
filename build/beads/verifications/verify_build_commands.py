#!/usr/bin/env fuchsia-vendored-python

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Verifies build commands for a list of GN and Bazel target pairs defined in a manifest file.
"""

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import typing as T

_DEBUG = False

_FUCHSIA_DIR = pathlib.Path(__file__).parent.parent.parent.parent

sys.path.insert(0, str(_FUCHSIA_DIR / "build/bazel/scripts"))
import build_utils

sys.path.insert(0, str(_FUCHSIA_DIR / "build/beads/scripts"))
import build_command_query_utils
import normalize_rustc_args
import path_normalizer
import shell_utils


def debug(s: T.Any) -> None:
    if _DEBUG:
        print(f"DEBUG: {s}", file=sys.stderr)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify GN and Bazel build commands for a list of targets."
    )

    build_utils.BuildPaths.add_parser_arguments(parser)

    parser.add_argument(
        "--manifest",
        required=True,
        type=pathlib.Path,
        help="Path to the manifest file (JSON)",
    )
    parser.add_argument(
        "--read_response_files",
        action="store_true",
        default=False,
        help="Read response files directly from the Bazel execroot instead of querying Starlark providers.",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        default=False,
        help="Print verbose output",
    )
    parser.add_argument(
        "--temp_dir", type=pathlib.Path, help="Temporary directory path"
    )
    parser.add_argument("--stamp", type=pathlib.Path, help="Stamp file path")

    args = parser.parse_args()

    global _DEBUG
    _DEBUG = args.verbose

    try:
        paths = build_utils.BuildPaths.from_parser_args(args)
    except ValueError as e:
        parser.error(str(e))

    build_command_query_utils.set_debug(args.verbose)

    with open(args.manifest) as f:
        targets = json.load(f)

    # TODO(https://fxbug.dev/502754609): Add support for clang targets.
    gn_labels = [t["gn"] for t in targets if t["type"] == "rustc"]
    bazel_labels = [t["bazel"] for t in targets if t["type"] == "rustc"]

    if not gn_labels or not bazel_labels:
        print("No targets to compare.")
        return 0

    debug(f"Fuchsia Dir: {paths.fuchsia_dir}")
    debug(f"Build Dir: {paths.build_dir}")
    debug(f"Manifest Path: {args.manifest}")
    debug(f"GN labels: {gn_labels}")
    debug(f"Bazel labels: {bazel_labels}")

    ninja_runner = build_utils.NinjaRunner(paths.ninja_path, paths.build_dir)
    bazel_paths = build_utils.BazelPaths(paths.fuchsia_dir, paths.build_dir)
    bazel_launcher = build_utils.BazelLauncher(bazel_paths.launcher)

    (
        gn_cmds_map,
        bazel_cmds_map,
    ) = build_command_query_utils.query_ninja_and_bazel_commands(
        gn_labels,
        bazel_labels,
        ninja_runner,
        bazel_launcher,
        bazel_paths.execroot,
        read_response_files=args.read_response_files,
    )

    temp_dir = tempfile.mkdtemp(
        prefix="verify_build_commands_", dir=args.temp_dir
    )
    all_success = True
    for idx, target in enumerate(targets):
        if target["type"] != "rustc":
            continue

        gn_label = target["gn"]
        bazel_label = target["bazel"]

        gn_cmd_raw = gn_cmds_map.get(gn_label, "")
        bazel_cmd_raw = bazel_cmds_map.get(bazel_label, "")

        if not gn_cmd_raw or not bazel_cmd_raw:
            print(
                f"Failed to get GN or Bazel rustc command for {gn_label} vs {bazel_label}."
            )
            all_success = False
            continue

        gn_cmd = shell_utils.ShellCommand(gn_cmd_raw)
        bazel_cmd = shell_utils.ShellCommand(bazel_cmd_raw)

        gn_rustc_cmd = shell_utils.find_command_with_tool(
            gn_cmd.split(), "rustc"
        )
        bazel_rustc_cmd = shell_utils.find_command_with_tool(
            bazel_cmd.split(), "rustc"
        )

        if not gn_rustc_cmd or not bazel_rustc_cmd:
            print(
                f"Failed to get GN or Bazel rustc command for {gn_label} vs {bazel_label}."
            )
            all_success = False
            continue

        if _DEBUG:
            debug(f"GN raw rustc command:\n{gn_rustc_cmd}\n")
            debug(f"Bazel raw rustc command:\n{bazel_rustc_cmd}\n")

        gn_path_normalizer = path_normalizer.GnPathNormalizer(
            paths.fuchsia_dir, paths.build_dir
        )
        bazel_path_normalizer = path_normalizer.BazelPathNormalizer(bazel_paths)

        normalized_gn_args = normalize_rustc_args.normalize_rustc_cmd(
            str(gn_rustc_cmd), gn_path_normalizer
        )
        normalized_bazel_args = normalize_rustc_args.normalize_rustc_cmd(
            str(bazel_rustc_cmd), bazel_path_normalizer
        )

        if _DEBUG:
            debug(f"GN normalized rustc command:\n{normalized_gn_args}\n")
            debug(f"Bazel normalized rustc command:\n{normalized_bazel_args}\n")

        if normalized_gn_args != normalized_bazel_args:
            debug(f"Mismatch for {gn_label} vs {bazel_label}!")
            gn_file = os.path.join(temp_dir, f"normalized_gn_args_{idx}.txt")
            bazel_file = os.path.join(
                temp_dir, f"normalized_bazel_args_{idx}.txt"
            )
            with open(gn_file, "w") as f:
                f.write("\n".join(normalized_gn_args) + "\n")
            with open(bazel_file, "w") as f:
                f.write("\n".join(normalized_bazel_args) + "\n")

            print(f"Mismatch for {gn_label} vs {bazel_label}!")
            print(f"diff -u {gn_file} {bazel_file}")
            subprocess.run(["diff", "-u", gn_file, bazel_file])

            all_success = False
        else:
            debug(f"Match for {gn_label} vs {bazel_label}")

    if not (_DEBUG or args.temp_dir):
        shutil.rmtree(temp_dir, ignore_errors=True)

    if all_success and args.stamp:
        with open(args.stamp, "w") as f:
            f.write("")

    return 0 if all_success else 1


if __name__ == "__main__":
    sys.exit(main())
