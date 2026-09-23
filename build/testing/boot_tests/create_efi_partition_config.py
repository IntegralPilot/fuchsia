#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys

USAGE = """Creates simple partition config that specifies an EFI disk (or
filesystem) image as the contents of a bootloader partition.
"""


def main():
    parser = argparse.ArgumentParser(usage=USAGE)
    parser.add_argument(
        "--metadata",
        metavar="FILE",
        help="Path at which to find the efi_input metadata JSON file",
        required=True,
    )
    parser.add_argument(
        "--output",
        metavar="FILE",
        help="Path at which to output the partition config file",
        required=True,
    )
    parser.add_argument(
        "--hardware-revision",
        help="The 'hardware revision' value of the config",
        required=True,
    )

    args = parser.parse_args()
    with open(args.metadata) as f:
        metadata = json.load(f)

    partitions = []
    for entry in metadata:
        partitions.append(
            {
                "image": entry["path"],
                "name": entry["name"],
                "type": entry["type"],
            }
        )

    config = {
        "bootloader_partitions": partitions,
        "bootstrap_partitions": [],
        "hardware_revision": args.hardware_revision,
        "partitions": [],
        "unlock_credentials": [],
    }

    with open(args.output, "w") as f:
        json.dump(config, f, indent=4)

    return 0


if __name__ == "__main__":
    sys.exit(main())
