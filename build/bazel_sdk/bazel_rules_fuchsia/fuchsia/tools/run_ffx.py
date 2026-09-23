#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import subprocess
from pathlib import Path


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
    parser = argparse.ArgumentParser(add_help=False)

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
        "--target",
        "-t",
        help="apply operations on a target",
    )

    return parser.parse_known_args()


def main():
    # Parse arguments.
    args, unknown = parse_args()

    ffx_flags = []
    if args.target:
        ffx_flags += ["--target", args.target]

    run(args.ffx, *ffx_flags, *unknown)


if __name__ == "__main__":
    main()
