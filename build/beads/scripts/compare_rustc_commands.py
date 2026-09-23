#!/usr/bin/env fuchsia-vendored-python

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import typing as T

import build_command_query_utils
import build_utils
import normalize_rustc_args
import path_normalizer
import shell_utils

# Enable debug logging.
_DEBUG = False

# Root directory of the Fuchsia source tree.
_FUCHSIA_DIR = pathlib.Path(__file__).parent.parent.parent.parent

# Path to the default Ninja binary.
_DEFAULT_NINJA_BIN = _FUCHSIA_DIR / "prebuilt/third_party/ninja/linux-x64/ninja"

sys.path.insert(0, str(_FUCHSIA_DIR / "build/bazel/scripts"))


def debug(s: T.Any) -> None:
    if _DEBUG:
        print(f"DEBUG: {s}", file=sys.stderr)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare GN and Bazel build commands for rustc."
    )

    build_utils.BuildPaths.add_parser_arguments(parser)

    parser.add_argument(
        "--gn_label", required=True, help="GN Rust target label"
    )
    parser.add_argument(
        "--bazel_label", required=True, help="Bazel Rust target label"
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

    args = parser.parse_args()

    global _DEBUG
    _DEBUG = args.verbose
    build_command_query_utils.set_debug(args.verbose)

    try:
        paths = build_utils.BuildPaths.from_parser_args(args)
    except ValueError as e:
        parser.error(str(e))

    debug(f"Fuchsia Dir: {paths.fuchsia_dir}")
    debug(f"Build Dir: {paths.build_dir}")
    debug(f"GN Label: {args.gn_label}")
    debug(f"Bazel Label: {args.bazel_label}")

    ninja_runner = build_utils.NinjaRunner(paths.ninja_path, paths.build_dir)

    bazel_paths = build_utils.BazelPaths(paths.fuchsia_dir, paths.build_dir)
    bazel_launcher = build_utils.BazelLauncher(bazel_paths.launcher)

    (
        gn_cmds_map,
        bazel_cmds_map,
    ) = build_command_query_utils.query_ninja_and_bazel_commands(
        [args.gn_label],
        [args.bazel_label],
        ninja_runner,
        bazel_launcher,
        bazel_paths.execroot,
        read_response_files=args.read_response_files,
    )

    gn_cmd = shell_utils.ShellCommand(gn_cmds_map.get(args.gn_label, ""))
    bazel_cmd = shell_utils.ShellCommand(
        bazel_cmds_map.get(args.bazel_label, "")
    )

    gn_rustc_cmd = shell_utils.find_command_with_tool(gn_cmd.split(), "rustc")
    bazel_rustc_cmd = shell_utils.find_command_with_tool(
        bazel_cmd.split(), "rustc"
    )

    debug("====== GN Command ======")
    debug(gn_rustc_cmd)
    debug("====== Bazel Command ======")
    debug(bazel_rustc_cmd)

    if not gn_rustc_cmd or not bazel_rustc_cmd:
        print("Failed to get GN or Bazel rustc command.")
        return 1

    gn_path_normalizer = path_normalizer.GnPathNormalizer(
        paths.fuchsia_dir, paths.build_dir
    )
    bazel_path_normalizer = path_normalizer.BazelPathNormalizer(bazel_paths)

    normalized_gn_args = normalize_rustc_args.normalize_rustc_cmd(
        str(gn_rustc_cmd), gn_path_normalizer
    )
    normalized_bazel_args = normalize_rustc_args.normalize_rustc_cmd(
        str(bazel_rustc_cmd),
        bazel_path_normalizer,
    )

    temp_dir = tempfile.mkdtemp(
        prefix="compare_rustc_commands_",
        dir=args.temp_dir,
    )
    gn_file = os.path.join(temp_dir, "normalized_gn_args.txt")
    bazel_file = os.path.join(temp_dir, "normalized_bazel_args.txt")
    with open(gn_file, "w") as f:
        f.write("\n".join(normalized_gn_args) + "\n")
    with open(bazel_file, "w") as f:
        f.write("\n".join(normalized_bazel_args) + "\n")

    debug(f"Comparing normalized args with command:")
    debug(f"diff -u {gn_file} {bazel_file}")
    result = subprocess.run(["diff", "-u", gn_file, bazel_file])

    # Preserve temporary results if verbose mode or a temp dir is specified.
    # In these modes, the user may want to inspect the temporary files.
    if not (_DEBUG or args.temp_dir):
        shutil.rmtree(temp_dir, ignore_errors=True)

    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
