#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys
import tarfile

# These names are from the tools included in the SDK.
# Excluding deprecated and internal tools.
# Eventually, it could be integrated into the build
# so the list is always updated.
WANT_NAMES = [
    "clidoc",
    "clidoc/_toc.yaml",
    "clidoc/bindc.md",
    "clidoc/blobfs-compression.md",
    "clidoc/cmc.md",
    "clidoc/configc.md",
    "clidoc/ffx.md",
    "clidoc/fidl-format.md",
    "clidoc/fidlc.md",
    "clidoc/fidlcat.md",
    "clidoc/fidlgen_cpp.md",
    "clidoc/fidlgen_hlcpp.md",
    "clidoc/funnel.md",
    "clidoc/minfs.md",
    "clidoc/symbolizer.md",
    "clidoc/zbi.md",
    "clidoc/zxdb.md",
]

# This is the list of subtools included in the SDK.
# It should match the output of `ffx commands` when run
# from the SDK. The list should not include in-tree only commands
# since it makes the output dependent on which tools are built.
WANT_FFX_SUBTOOLS = [
    "assembly",
    "audio",
    "component",
    "config",
    "coverage",
    "debug",
    "doctor",
    "driver",
    "emu",
    "fuzz",
    "inspect",
    "log",
    "net",
    "package",
    "platform",
    "process",
    "product",
    "profile",
    "repository",
    "scrutiny",
    "sdk",
    "session",
    "setui",
    "starnix",
    "target",
    "test",
    "trace",
    "version",
    "wlan",
]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Checks the contents of the clidoc tarball for correctness.",
    )
    parser.add_argument(
        "--input",
        type=str,
        required=True,
        help="Output location where generated docs should go",
    )
    parser.add_argument(
        "--output",
        type=str,
        required=True,
        help="Output location where the contents should be listed.",
    )

    args = parser.parse_args()

    if not os.path.exists(args.input):
        raise ValueError(f"{args.input} does not exist.")

    contents = check_clidoc_contents(args.input)
    check_ffx_contents(args.input)

    with open(args.output, "w") as f:
        f.write(f"{contents}")

    return 0


def check_clidoc_contents(tarball: str) -> list[str]:
    got_names = []
    with tarfile.open(tarball, mode="r:gz") as tf:
        got_names = [m.name for m in tf.getmembers()]
    # sort to make it stable
    got_names.sort()
    want_names = set(WANT_NAMES)
    for name in got_names:
        # Remove name from the set. If name is unexpected, that's ok (maybe update want_names?).
        if name in want_names:
            want_names.remove(name)
    if want_names:
        raise ValueError(f"Missing files from clidoc tarball: {want_names}")
    return got_names


def check_ffx_contents(tarball: str) -> None:
    want_subcommands = set(WANT_FFX_SUBTOOLS)

    with tarfile.open(tarball, mode="r:gz") as tf:
        ffx_member = tf.getmember("clidoc/ffx.md")
        if not ffx_member:
            raise ValueError("clidoc/ffx.md is missing from clidoc tarball.")
        ffx_file = tf.extractfile(ffx_member)
        contents = ffx_file.readlines() if ffx_file else []
    # filter the H2 elements these are subcommands
    subcommands_headings = [
        l.decode() for l in contents if l.startswith(b"## ")
    ]
    got_subcommands = [h[3:].split()[0] for h in subcommands_headings]

    for cmd in got_subcommands:
        if cmd in want_subcommands:
            want_subcommands.remove(cmd)

    if want_subcommands:
        want = list(want_subcommands)
        want.sort()
        got = list(got_subcommands)
        got.sort()
        raise ValueError(f"Missing ffx subcommands: {want} in {got}")


if __name__ == "__main__":
    sys.exit(main())
