#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import re
import subprocess
import sys

# Example: "zx_vmo_transfer_data T ---------------- 0"
PATTERN = re.compile(r"^([^ ]+) T ")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "nm",
        help="llvm-nm binary",
    )
    parser.add_argument(
        "out_retain",
        type=argparse.FileType("w"),
        help="Output list of symbols to be retained, one per line",
    )
    parser.add_argument(
        "out_undefined",
        type=argparse.FileType("w"),
        help="Output list of --undefined=SYMBOL directives, one per line",
    )
    parser.add_argument(
        "input",
        help="Path of the ELF library to be inspected",
    )
    parser.add_argument(
        "extra_symbols",
        nargs="*",
        help="Extra symbols to be exported",
    )
    args = parser.parse_args()

    exported_symbols = set(args.extra_symbols)

    nm_output = subprocess.check_output(
        [args.nm, "-gP", args.input],
        encoding="ascii",
    )
    for line in nm_output.splitlines():
        if m := PATTERN.search(line):
            exported_symbols.add(m.group(1))

    for symbol in sorted(exported_symbols):
        args.out_retain.write(f"{symbol}\n")
        args.out_undefined.write(f"--undefined={symbol}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
