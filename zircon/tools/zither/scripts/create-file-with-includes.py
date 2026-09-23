#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import sys

# Creates an empty C/C++/asm file with the given #includes.


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        help="Where to write the resulting main()-containing file",
        required=True,
    )
    parser.add_argument(
        "--comment",
        help="The file-level comment of the generated file",
        required=True,
    )
    parser.add_argument("includes", nargs="*", help="files to #include")
    args = parser.parse_args()

    contents = ["// %s" % args.comment]

    # Include headers before .inc files, as the latter will likely refer to
    # things defined in the former.
    for include in args.includes:
        if include.endswith(".h"):
            contents.append("#include <%s>" % include)
    for include in args.includes:
        if not include.endswith(".h"):
            contents.append("#include <%s>" % include)
    contents.append("")

    with open(args.output, "w") as output_file:
        output_file.write("\n".join(contents))

    return 0


if __name__ == "__main__":
    sys.exit(main())
