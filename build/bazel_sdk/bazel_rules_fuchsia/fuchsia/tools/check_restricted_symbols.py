# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tool to check that the binary is not exporting restricted symbols."""

import argparse
import os
import shutil
import subprocess
import sys


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary",
        help="Path to the binary file",
        required=True,
    )
    parser.add_argument(
        "--output",
        help="Path to the output file",
        required=True,
    )
    parser.add_argument(
        "--objdump",
        help="Path to the objdump binary",
        required=True,
    )
    parser.add_argument(
        "--restricted_symbols_file",
        help="Path to the restricted_symbols_file",
        required=True,
    )
    return parser.parse_args()


def run(*command):
    try:
        # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
        # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE
        # inheritance which can confuse child Python processes.
        env = dict(os.environ)
        env.pop("RUNFILES_DIR", None)
        env.pop("RUNFILES_MANIFEST_FILE", None)

        return subprocess.check_output(
            command,
            text=True,
            env=env,
        ).strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        raise e


def get_exported_symbols(args):
    # The output from objdump will contain a few lines before a line
    # with "DYNAMIC SYMBOL TABLE:"
    # We look for that line and then take the 5th column which is the symbol
    #
    # it will look something like
    #
    # DYNAMIC SYMBOL TABLE:
    # 0000000000000000      DF *UND*	0000000000000000 _zx_futex_wait
    # 0000000000000000      DF *UND*	0000000000000000 _zx_futex_wake
    contents = run(args.objdump, "-TC", args.binary).splitlines()

    first_row = -1
    for i, line in enumerate(contents):
        if "DYNAMIC SYMBOL TABLE:" in line:
            first_row = i + 1
            break

    symbols = []
    for line in contents[first_row:]:
        cols = line.split()
        symbols.append(cols[-1])

    return symbols


def verify_exported_symbols(symbols, args):
    with open(args.restricted_symbols_file) as f:
        restricted_symbols = [s.strip() for s in f.readlines()]

    found_restricted_symbols = []
    for symbol in symbols:
        if symbol in restricted_symbols:
            found_restricted_symbols.append(symbol)

    if len(found_restricted_symbols) > 0:
        print(
            f"FAIL: Found restricted symbols in {args.binary}",
            file=sys.stderr,
        )
        for symbol in found_restricted_symbols:
            print(f"FAIL: Found restricted symbol {symbol}", file=sys.stderr)
        sys.exit(1)


def main():
    args = parse_args()
    symbols = get_exported_symbols(args)
    verify_exported_symbols(symbols, args)

    # Copy the binary so that we can ensure this action
    # always runs.
    shutil.copy2(args.binary, args.output)


if __name__ == "__main__":
    sys.exit(main())
