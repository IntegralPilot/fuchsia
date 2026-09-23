#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(
        description="Create FIDL IR path directory. Symlinks to original file."
        "Intended to transition away from all_fidl_json.txt"
    )
    parser.add_argument(
        "--root-dir",
        help="The path to the root output directory",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--ir-path",
        help="The path to the original FIDL IR file which will be symlinked.",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--library-name",
        help="The name of the library, e.g. fuchsia.hwinfo",
        type=str,
        required=True,
    )
    args = parser.parse_args()
    out_dir = args.root_dir / args.library_name
    out_dir.mkdir(parents=True, exist_ok=True)
    out_file = out_dir / f"{args.library_name}.fidl.json"
    out_file.unlink(missing_ok=True)
    out_file.symlink_to(args.ir_path)


if __name__ == "__main__":
    main()
