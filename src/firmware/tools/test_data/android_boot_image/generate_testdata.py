#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Script to generate android_boot_image testdata.

Run this manually to re-generate the testdata.

This is pretty bad, it would be far better to automatically generate it at build time
or test runtime, but I'm having trouble doing it that way. For now this is sufficient.
"""

import pathlib
import subprocess
import tempfile

_MY_DIR = pathlib.Path(__file__).parent

_FUCHSIA_ROOT_DIR = _MY_DIR.parents[4]
_MKBOOTIMG_PATH = (
    _FUCHSIA_ROOT_DIR
    / "zircon"
    / "third_party"
    / "tools"
    / "android"
    / "mkbootimg"
)


def main() -> None:
    with tempfile.TemporaryDirectory() as temp_dir_str:
        temp_dir = pathlib.Path(temp_dir_str)

        (temp_dir / "kernel").write_bytes(b"test kernel contents")
        (temp_dir / "ramdisk").write_bytes(b"test ramdisk contents")
        (temp_dir / "dtb").write_bytes(b"test dtb contents")

        # v2 boot image
        command = (
            [_MKBOOTIMG_PATH]
            + ["--header_version", "2"]
            + ["--pagesize", "4096"]
            + ["--kernel", temp_dir / "kernel"]
            + ["--ramdisk", temp_dir / "ramdisk"]
            + ["--dtb", temp_dir / "dtb"]
            + ["-o", _MY_DIR / "test_boot_image_v2.bin"]
        )
        subprocess.run(command, check=True)

        # v4 boot image
        command = (
            [_MKBOOTIMG_PATH]
            + ["--header_version", "4"]
            + ["--pagesize", "4096"]
            + ["--kernel", temp_dir / "kernel"]
            + ["--ramdisk", temp_dir / "ramdisk"]
            + ["-o", _MY_DIR / "test_boot_image_v4.bin"]
        )
        subprocess.run(command, check=True)

        # v4 vendor boot image
        command = (
            [_MKBOOTIMG_PATH]
            + ["--header_version", "4"]
            + ["--pagesize", "4096"]
            + ["--dtb", temp_dir / "dtb"]
            + ["--vendor_boot", _MY_DIR / "test_vendor_boot_image_v4.bin"]
        )
        subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
