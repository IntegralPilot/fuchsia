#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generate a GPT image for bootstrapping NUC"""

import argparse
import pathlib
import subprocess
import sys
import tempfile

_SCRIPT_DIR = pathlib.Path(__file__).parent.resolve()
_FUCHSIA_ROOT = _SCRIPT_DIR.parent.parent.parent.parent.parent
_CGPT = _FUCHSIA_ROOT / "prebuilt" / "tools" / "cgpt" / "linux-x64" / "cgpt"

_SZ_MB = 1024 * 1024
_BLOCK_SIZE = 512
_FIRST = 34  # MBR + GPT header + GPT entries

# Partition information taken mostly from src/firmware/paver/x64.cc
PARTITIONS = [
    ("bootloader", "5ECE94FE-4C86-11E8-A15B-480FCF35F8E6", 128 * _SZ_MB),
    ("zircon_a", "9B37FFF6-2E58-466A-983A-F7926D0B04E0", 128 * _SZ_MB),
    ("zircon_b", "9B37FFF6-2E58-466A-983A-F7926D0B04E0", 128 * _SZ_MB),
    ("zircon_r", "9B37FFF6-2E58-466A-983A-F7926D0B04E0", 196 * _SZ_MB),
    ("vbmeta_a", "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9", 64 * 1024),
    ("vbmeta_b", "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9", 64 * 1024),
    ("vbmeta_r", "421A8BFC-85D9-4D85-ACDA-B64EEC0133E9", 64 * 1024),
    ("durable_boot", "A409E16B-78AA-4ACC-995C-302352621A41", 4 * 1024),
    ("fvm", "49FD7CB8-DF15-4E73-B9D9-992070127F0F", 56 * 1024 * _SZ_MB),
]


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("out", help="path to output file")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    with tempfile.TemporaryDirectory() as tempdir:
        tempdir = pathlib.Path(tempdir)
        disk_file = tempdir / "disk.img"
        total = sum([ele[2] for ele in PARTITIONS])
        total += (1 + 33 * 2) * _BLOCK_SIZE
        with open(disk_file, "wb") as f:
            f.truncate(total)
        subprocess.run([_CGPT, "create", f"{disk_file}"], check=True)
        subprocess.run([_CGPT, "boot", "-p", f"{disk_file}"], check=True)
        off = _FIRST
        for name, type, sz in PARTITIONS:
            sz = sz // _BLOCK_SIZE
            subprocess.run(
                [
                    _CGPT,
                    "add",
                    "-b",
                    f"{off}",
                    "-s",
                    f"{sz}",
                    "-t",
                    type,
                    "-l",
                    f"{name}",
                    f"{disk_file}",
                ],
                check=True,
            )
            off += sz
        # Extracts the primary GPT
        pathlib.Path(args.out).write_bytes(
            disk_file.read_bytes()[: _FIRST * _BLOCK_SIZE]
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
