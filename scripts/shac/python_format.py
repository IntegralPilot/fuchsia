#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(
        description="Chain Python formatting tools."
    )
    parser.add_argument(
        "--python", required=True, help="Path to python interpreter"
    )
    parser.add_argument(
        "--autoflake", required=True, help="Path to autoflake script"
    )
    parser.add_argument("--isort", required=True, help="Path to isort script")
    parser.add_argument("--black", required=True, help="Path to black binary")
    parser.add_argument(
        "--pyproject-toml", required=True, help="Path to pyproject.toml"
    )
    parser.add_argument("file", help="File to format")

    args = parser.parse_args()

    # 1. Run autoflake
    autoflake_cmd = [
        args.python,
        args.autoflake,
        "--remove-unused-variables",
        "--remove-all-unused-imports",
        "--remove-duplicate-keys",
        "--ignore-init-module-imports",
        "--stdout",
        args.file,
    ]
    res_autoflake = subprocess.run(autoflake_cmd, capture_output=True)
    if res_autoflake.returncode != 0:
        sys.stderr.buffer.write(res_autoflake.stderr)
        sys.exit(res_autoflake.returncode)

    # 2. Run isort on autoflake output
    isort_cmd = [
        args.python,
        args.isort,
        "--skip",
        ".venvs",
        "--stdout",
        "--filename",
        args.file,
        "-",
    ]
    res_isort = subprocess.run(
        isort_cmd, input=res_autoflake.stdout, capture_output=True
    )
    if res_isort.returncode != 0:
        sys.stderr.buffer.write(res_isort.stderr)
        sys.exit(res_isort.returncode)

    # 3. Run black on isort output
    black_cmd = [
        args.black,
        "--config",
        args.pyproject_toml,
        "-",
    ]
    res_black = subprocess.run(
        black_cmd, input=res_isort.stdout, capture_output=True
    )
    if res_black.returncode != 0:
        sys.stderr.buffer.write(res_black.stderr)
        sys.exit(res_black.returncode)

    sys.stdout.buffer.write(res_black.stdout)


if __name__ == "__main__":
    main()
