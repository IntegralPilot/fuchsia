#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys
import xml.etree.ElementTree

PACKAGES = [
    "fuchsia/firmware/arm-trusted-firmware-qemu-bios",
    "fuchsia/third_party/qemu/${platform}",
    "fuchsia/third_party/android/aemu/release/${platform}",
    "fuchsia/third_party/android/aemu/release-gfxstream/${platform}",
    "fuchsia/third_party/crosvm/${platform}",
    "fuchsia/third_party/edk2",
]


def parse_snapshot(file):
    return xml.etree.ElementTree.parse(file).getroot()


def find_package(snapshot, name):
    return snapshot.find("./packages/package[@name='%s']" % name)


def generate_prebuilt_versions(jiri_snapshot, output_file):
    prebuilt_versions = []
    snapshot = parse_snapshot(jiri_snapshot)
    for package in PACKAGES:
        package_info = find_package(snapshot, package)
        prebuilt_versions.append(
            {
                "name": package_info.attrib["name"],
                "version": package_info.attrib["version"],
            },
        )
    with open(output_file, "w") as f:
        json.dump(prebuilt_versions, f, indent=2)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--jiri-snapshot",
        help="Path to the jiri snapshot",
        required=True,
    )
    parser.add_argument(
        "--output",
        help="Path to the output file to write the emulator prebuilt versions to in JSON format",
        required=True,
    )
    args = parser.parse_args()

    generate_prebuilt_versions(args.jiri_snapshot, args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
