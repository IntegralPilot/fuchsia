#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Check very quickly whether the regenerator inputs have changed."""

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))
import workspace_utils


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "ninja_build_dir", type=Path, help="Ninja build directory."
    )
    parser.add_argument(
        "--quiet", action="store_true", help="Do not print anything."
    )
    args = parser.parse_args()

    updated_inputs = workspace_utils.check_regenerator_inputs_updates(
        args.ninja_build_dir
    )
    if not updated_inputs:
        if not args.quiet:
            print("up-to-date")
        return 0

    if not args.quiet:
        print("regeneration required!")
        for path in updated_inputs:
            print(os.path.relpath(args.ninja_build_dir / path))

    return 1


if __name__ == "__main__":
    sys.exit(main())
