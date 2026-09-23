#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
validate that vmzircon does not contain banned symbols

If vmzircon contains no guard variables for function scoped statics,
generate a depfile and exit with 0.  Otherwise, print the mangled
symbol names for function scoped static guard variables and exit with
a non-zero result.

"""

import argparse
import os
import subprocess
import sys

# Guard variables for function scoped statics start with _ZGVZ.
BANNED_PREFIX = b"_ZGVZ"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("nm_bin", help="path to nm binary")
    parser.add_argument(
        "vmzircon_rsp",
        help="path to a file containing the path to vmzircon",
    )
    parser.add_argument("output", help="path to the output file to create")
    parser.add_argument("depfile", help="path to the depfile to generate")
    args = parser.parse_args()

    # Read the path to vmzircon.
    with open(args.vmzircon_rsp) as zircon_rsp_elf:
        vmzircon = zircon_rsp_elf.read().rstrip("\n")

    # Write the depfile.
    with open(args.depfile, "w") as depfile:
        print(
            f"{args.output:s}: {args.vmzircon_rsp:s} {vmzircon:s}",
            file=depfile,
        )

    # Create a list of guard variables for function scoped statics.
    nm = subprocess.Popen(
        [args.nm_bin, "-j", vmzircon],
        stdout=subprocess.PIPE,
    )
    banned_guard_variables = list(
        map(
            lambda x: x.decode("UTF-8").rstrip("\n"),
            filter(lambda x: x.startswith(BANNED_PREFIX), nm.stdout),
        ),
    )

    if len(banned_guard_variables) > 0:
        print(
            f"{parser.prog:s}: ERROR: {vmzircon:s} contains non-trivial function scoped statics. Mangled guard variable symbol names follow:",
        )
        print(*banned_guard_variables, sep="\n")
        sys.exit(1)

    # None found.  Write an empty output file.
    with open(args.output, "w") as file:
        os.utime(file.name, None)


if __name__ == "__main__":
    main()
