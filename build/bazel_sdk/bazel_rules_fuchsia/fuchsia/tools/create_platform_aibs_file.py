# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Create platform AIB list file"""

import argparse
import json
import os


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--platform-aibs",
        help="Path to platform AIBs directory",
        required=True,
    )
    parser.add_argument(
        "--output",
        help="output platform_aibs.json file",
        required=True,
    )
    return parser.parse_args()


def main():
    args = parse_args()

    paths = []
    for subdir in os.listdir(args.platform_aibs):
        dir_path = os.path.join(args.platform_aibs, subdir)
        if not os.path.isdir(dir_path):
            continue
        paths.append({"path": dir_path})

    with open(args.output, "w") as f:
        json.dump(paths, f, indent=2)


if __name__ == "__main__":
    main()
