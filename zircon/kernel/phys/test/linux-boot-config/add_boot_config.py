#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Given an Input ramdisk appends content to it, including a trailer for bootconfig.

This script does not parse or validate the BOOTCONFIG file to be appended, it assumes to be correct,
since the purpose is to generate data for testing. The initrd in this case is a ZBI, or may be something else.
"""

import argparse
import ctypes
import os
import shutil
import struct

LINUX_BOOT_CONFIG_TRAILER_MAGIC = "#BOOTCONFIG\n"
LINUX_BOOT_CONFIG_TRAILER_MAGIC_BYTES = LINUX_BOOT_CONFIG_TRAILER_MAGIC.encode(
    "ascii"
)[0:12]
LINUX_BOOT_CONFIG_TRAILER_SIZE = 20
LINUX_BOOT_CONFIG_CONTENTS_SIZE_ALIGNMENT = 4

LINUX_BOOT_CONFIG_TRAILER_SIZE_OFFSET = 0
LINUX_BOOT_CONFIG_TRAILER_CHECKSUM_OFFSET = 4
LINUX_BOOT_CONFIG_TRAILER_MAGIC_OFFSET = 8

PADDING_BYTES = bytearray(b"\0\0\0\0")

PACKED_U32 = struct.Struct("<I")
U32_SIZE = 4


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--input-initrd",
        type=argparse.FileType("rb"),
        help="Input initrd. May or not already contain BOOTCONFIG",
        required=True,
    )
    parser.add_argument(
        "--boot-config",
        type=argparse.FileType("rb"),
        help="Bootconfig file to append to `input-initrd`.",
        required=True,
    )
    parser.add_argument(
        "--output-initrd",
        type=argparse.FileType("wb"),
        help="Output initrd. Contains the boot config contents",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        type=argparse.FileType("w"),
        help="Output depfile for ninja",
        required=True,
    )
    parser.add_argument(
        "--output-header",
        type=argparse.FileType("w"),
        help="Generate C++ header with the boot config file inlined as ma constant kExpectedBootConfigContents.",
        required=True,
    )
    parser.add_argument(
        "--corrupt-checksum",
        action="store_true",
        help="When set will corrupt the checksum of the generated trailer",
        required=False,
    )
    parser.add_argument(
        "--corrupt-size",
        action="store_true",
        help="When set will corrupt the size of the generated trailer",
        required=False,
    )
    args = parser.parse_args()

    shutil.copyfileobj(args.input_initrd, args.output_initrd)

    # Checksum for BOOTCONFIG is just the sum of the bytes.
    checksum = 0
    # Size including padding bytes, size must be aligned to 4 bytes.
    size = 0

    # Check if input-initrd has the "BOOTCONFIG\n" magic trailer.
    args.input_initrd.seek(0, os.SEEK_END)
    initrd_size = args.input_initrd.tell()
    # We need the last 20 bytes (trailer)
    if initrd_size >= LINUX_BOOT_CONFIG_TRAILER_SIZE:
        args.input_initrd.seek(
            initrd_size - LINUX_BOOT_CONFIG_TRAILER_SIZE, os.SEEK_SET
        )
        trailer = args.input_initrd.read()
        # We have a trailer
        if trailer[
            LINUX_BOOT_CONFIG_TRAILER_MAGIC_OFFSET:LINUX_BOOT_CONFIG_TRAILER_SIZE
        ] == LINUX_BOOT_CONFIG_TRAILER_MAGIC.encode("ascii"):
            size = PACKED_U32.unpack(
                trailer[LINUX_BOOT_CONFIG_TRAILER_SIZE_OFFSET:U32_SIZE]
            )[0]
            checksum = PACKED_U32.unpack(
                trailer[LINUX_BOOT_CONFIG_TRAILER_CHECKSUM_OFFSET:U32_SIZE]
            )[0]

            # Adjust size based on possible padding bytes.
            padding_bytes_max = min(
                initrd_size - LINUX_BOOT_CONFIG_TRAILER_SIZE,
                LINUX_BOOT_CONFIG_CONTENTS_SIZE_ALIGNMENT,
            )
            args.input_initrd.seek(-LINUX_BOOT_CONFIG_TRAILER_SIZE, os.SEEK_CUR)
            possible_padded_bytes = args.input_initrd.read(padding_bytes_max)
            actual_padded = 0
            for c in reversed(possible_padded_bytes):
                if c != 0:
                    break
                actual_padded += 1
            bootconfig_start = (
                initrd_size - LINUX_BOOT_CONFIG_TRAILER_SIZE - actual_padded
            )
            size -= actual_padded
        else:
            # Append at the end of the file
            bootconfig_start = initrd_size

    # Truncate to the start of bootconfig, which would have removed the trailer.
    args.output_initrd.truncate(bootconfig_start)

    appended_boot_config = bytearray(args.boot_config.read())
    # Round up to next multiple of 4.
    padding_bytes = 4 * (
        int((bootconfig_start + len(appended_boot_config) + 3) / 4)
    ) - (bootconfig_start + len(appended_boot_config))

    appended_boot_config.extend(PADDING_BYTES[0:padding_bytes])

    for b in appended_boot_config:
        checksum += b
    size += len(appended_boot_config)
    # Remove the old trailer, and now we can append.
    args.output_initrd.seek(0, os.SEEK_END)
    args.output_initrd.write(appended_boot_config)

    if args.corrupt_size:
        # Include the size of the initrd AND the trailer.
        size += initrd_size + LINUX_BOOT_CONFIG_TRAILER_SIZE
        appended_boot_config.clear()

    if args.corrupt_checksum:
        checksum += 1
        appended_boot_config.clear()

    # Trailer u32 u32 #BOOTCONFIG\n
    # `struct.pack` will raise an error if their input is not within the range of u32,
    #
    # using `ctypes.c_uint32`` will do the wraparound(both overflow and negative integers).
    # `struct.pack`.
    packed_size = PACKED_U32.pack(ctypes.c_uint32(size).value)
    packed_checksum = PACKED_U32.pack(ctypes.c_uint32(checksum).value)
    args.output_initrd.write(packed_size)
    args.output_initrd.write(packed_checksum)
    args.output_initrd.write(bytearray(LINUX_BOOT_CONFIG_TRAILER_MAGIC_BYTES))

    # Generate a Cpp file the contents of the file embedded as a constant, such that test infrastructure can validate that things
    # are handed over correctly.
    args.output_header.write(
        f"""#pragma once
#include <string_view>
using namespace std::literals;
inline constexpr std::string_view kExpectedBootConfigContents = R\"'({appended_boot_config.decode("ascii")})'\"sv;
"""
    )
    args.depfile.write(
        "{} {}: {} {}\n".format(
            args.output_initrd.name,
            args.output_header,
            args.input_initrd.name,
            args.boot_config.name,
        )
    )


if __name__ == "__main__":
    main()
