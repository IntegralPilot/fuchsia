#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Verify that there are no duplicate headers."""

import argparse
import os
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gn-label", required=True)
    parser.add_argument("--stamp-file", required=True)
    parser.add_argument("headers", nargs="*")
    args = parser.parse_args()
    unique_headers = set(args.headers)
    if len(unique_headers) != len(args.headers):
        print(
            "ERROR: Found duplicate headers for target %s:" % args.gn_label,
            file=sys.stderr,
        )
        for header in args.headers:
            if args.headers.count(header) > 1:
                print("- " + header, file=sys.stderr)
        return 1

    os.makedirs(os.path.dirname(args.stamp_file), exist_ok=True)
    with open(args.stamp_file, "w") as f:
        f.write("1")
    return 0


if __name__ == "__main__":
    sys.exit(main())
