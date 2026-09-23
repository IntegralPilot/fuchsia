#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Wraps a ZBI inside an Android boot image.

This script works in tandem with the boot shim to allow booting Fuchsia on Android
bootloaders.

The general idea is that our boot shim goes in the kernel and our ZBI goes in the
ramdisk, so the boot chain looks like bootloader -> boot shim -> ZBI.
"""

import argparse
import pathlib
import subprocess
import sys


def create_android_boot_image(
    mkbootimg: pathlib.Path,
    boot_shim: pathlib.Path,
    zbi: pathlib.Path,
    out: pathlib.Path,
) -> None:
    subprocess.run(
        [sys.executable, mkbootimg]
        + ["--kernel", boot_shim]
        + ["--ramdisk", zbi]
        + ["--header_version", "4"]
        + ["--pagesize", "4096"]
        + ["--output", out],
        check=True,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mkbootimg",
        required=True,
        type=pathlib.Path,
        help="Path to Android `mkbootimg` Python tool",
    )
    parser.add_argument(
        "--boot-shim",
        required=True,
        type=pathlib.Path,
        help="Path to boot shim",
    )
    parser.add_argument(
        "-z", "--zbi", required=True, type=pathlib.Path, help="Path to ZBI"
    )
    parser.add_argument(
        "-o",
        "--output",
        required=True,
        type=pathlib.Path,
        help="Path to output file",
    )

    return parser.parse_args()


def main():
    args = parse_args()

    create_android_boot_image(
        args.mkbootimg, args.boot_shim, args.zbi, args.output
    )


if __name__ == "__main__":
    main()
