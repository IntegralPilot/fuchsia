#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT
"""
This writes out a response file for rustc the might contain a --cfg switch.
It reads a response file containing the name of the internal ZBI kernel image
file, and compares that image's load time memory requirement to a threshold.
"""

import argparse
import os
import struct
import sys
from collections import namedtuple

BIG_KERNEL_SIZE = 7 << 20

ZBI_HEADER_FORMAT = struct.Struct("<IIIIIIII")
ZBI_KERNEL_FORMAT = struct.Struct("<QQ")


def unpack_header(format, file):
    return format.unpack(file.read(format.size))


class ZbiHeader(
    namedtuple(
        "ZbiHeader",
        [
            "type",
            "length",
            "extra",
            "flags",
            "reserved0",
            "reserved1",
            "magic",
            "crc32",
        ],
    )
):
    def __new__(cls, file):
        return cls._make(unpack_header(ZBI_HEADER_FORMAT, file))


class KernelHeader(namedtuple("ZbiHeader", ["entry", "reserve_memory_size"])):
    def __new__(cls, file):
        return cls._make(unpack_header(ZBI_KERNEL_FORMAT, file))


def kernel_memory_size(filename):
    with open(filename, "rb") as f:
        container = ZbiHeader(f)
        kernel = KernelHeader(f)
        load_size = ZBI_HEADER_FORMAT.size + container.length
        return load_size + kernel.reserve_memory_size


def write_if_changed(filename, contents):
    if os.path.exists(filename):
        with open(filename) as f:
            if f.read() == contents:
                return
    with open(filename, "w") as f:
        f.write(contents)


def write_depfile(depfile, output, *inputs):
    input_list = " ".join(list(inputs))
    with open(depfile, "w") as f:
        f.write(f"{output}: {input_list}\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--depfile",
        metavar="FILE",
        required=True,
        help="Write depfile for inputs used",
    )
    parser.add_argument(
        "--rust-cfg-rspfile",
        metavar="FILE",
        required=True,
        help="Write output file of `--cfg` rustc switch",
    )
    parser.add_argument(
        "rspfile",
        metavar="FILE",
        nargs=1,
        help="Read name of kernel image file from FILE",
    )
    args = parser.parse_args()

    with open(args.rspfile[0]) as rspfile:
        kernel_image_file = rspfile.read().strip()

    size = kernel_memory_size(kernel_image_file)
    big = size > BIG_KERNEL_SIZE

    rust_cfg = '--cfg=feature="big_zircon_kernel"\n' if big else ""

    write_depfile(
        args.depfile, args.rust_cfg_rspfile, *args.rspfile, kernel_image_file
    )

    write_if_changed(args.rust_cfg_rspfile, rust_cfg)

    return 0


if __name__ == "__main__":
    sys.exit(main())
