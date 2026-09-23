#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import subprocess
from pathlib import Path

from fuchsia.tools.fuchsia_task_lib import Terminal


def run(*command) -> None:
    try:
        # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
        # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
        # inheritance which can confuse child Python processes.
        env = dict(os.environ)
        env.pop("RUNFILES_DIR", None)
        env.pop("RUNFILES_MANIFEST_FILE", None)

        subprocess.check_call(
            " ".join([str(arg) for arg in command]),
            env=env,
            shell=True,
        )
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        raise e


def parse_args() -> None:
    """Parses arguments."""
    parser = argparse.ArgumentParser()

    def path_arg(type="file"):
        def arg(path):
            path = Path(path)
            if (
                type == "file"
                and not path.is_file()
                or type == "directory"
                and not path.is_dir()
            ):
                print(f'Path "{path}" is not a {type}!')
            return path

        return arg

    parser.add_argument(
        "--ffx",
        type=path_arg(),
        help="A path to the ffx tool.",
        required=True,
    )
    parser.add_argument(
        "--build-id-dirs",
        help="Paths to the build_id directories.",
        nargs="*",
        type=path_arg("directory"),
        required=True,
    )
    parser.add_argument(
        "--build-dirs",
        help=(
            "Build directories corresponding with build_id_dirs used by zxdb "
            "to locate the source code. If a file is passed in, the directory "
            "containing that file will be used instead."
        ),
        nargs="*",
        type=str,
        required=True,
    )

    return parser.parse_args()


def main():
    # Parse arguments.
    args = parse_args()

    if not args.build_id_dirs:
        print(Terminal.warn("No debug symbols to register."))

    for build_id_dir, _build_dir in zip(args.build_id_dirs, args.build_dirs):
        build_dir_args = []
        build_dir = Path(os.getenv(_build_dir, _build_dir)).resolve()
        if build_dir.is_file():
            build_dir = build_dir.parent
        if build_dir.is_dir():
            build_dir_args = ["--build-dir", build_dir]
        else:
            print(Terminal.error(f"Invalid build directory {_build_dir}"))
        run(
            args.ffx,
            "debug",
            "symbol-index",
            "add",
            build_id_dir,
            *build_dir_args,
        )


if __name__ == "__main__":
    main()
