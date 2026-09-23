#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Formats JSON files using standard indentation and sorted keys."""

import argparse
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description="Pretty-print JSON files.")
    parser.add_argument(
        "file",
        type=Path,
        nargs="+",
        help="JSON file to format in-place.",
    )
    parser.add_argument(
        "--sort-keys",
        default=True,
        action=argparse.BooleanOptionalAction,
        dest="sort_keys",
        help="Sort object keys.",
    )
    parser.add_argument(
        "--quiet",
        default=False,
        action=argparse.BooleanOptionalAction,
        dest="quiet",
        help="Suppress output.",
    )

    args = parser.parse_args()
    for json_path in args.file:
        try:
            with open(json_path, "r", encoding="utf-8") as f:
                original = f.read()
            data = json.loads(original)
            formatted = json.dumps(
                data,
                indent=4,
                sort_keys=args.sort_keys,
                separators=(",", ": "),
            )
            new_content = formatted + "\n"
            if original != new_content:
                temp_file = None
                try:
                    with tempfile.NamedTemporaryFile(
                        "w",
                        encoding="utf-8",
                        dir=json_path.parent,
                        delete=False,
                    ) as tf:
                        temp_file = tf.name
                        tf.write(new_content)
                    shutil.copymode(json_path, temp_file)
                    os.replace(temp_file, json_path)
                finally:
                    if temp_file and os.path.exists(temp_file):
                        os.unlink(temp_file)
        except json.JSONDecodeError as e:
            if not args.quiet:
                print(f"JSON decode error in {json_path}: {e}", file=sys.stderr)
            sys.exit(1)


if __name__ == "__main__":
    main()
