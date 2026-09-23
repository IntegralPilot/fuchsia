#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A script to check for missing LICENSE files when vendoring Rust crates

# Should be run from top-level of third_party/rust_crates repository.

import argparse
import os
import sys


def die(reason):
    raise Exception(reason)


def check_subdir_license(subdir):
    if subdir.startswith(".") or not os.path.isdir(subdir):
        return

    license_files = [
        file
        for file in os.listdir(subdir)
        if file.startswith("LICENSE")
        or file.startswith("LICENCE")
        or file.startswith("license")
    ]
    if not license_files:  # FIXME better way to track whether one exists
        die("Missing license for %s" % subdir)


def check_licenses(directory):
    success = True
    os.chdir(directory)
    subdirs = sorted(os.listdir(os.getcwd()))
    for subdir in subdirs:
        try:
            check_subdir_license(subdir)
        except Exception as err:
            print("ERROR    %s" % err)
            success = False
    print("Done checking licenses.")
    return success


def main():
    parser = argparse.ArgumentParser(
        "Verifies licenses for third-party Rust crates"
    )
    parser.add_argument(
        "--directory",
        help="Directory containing the crates",
        default=os.getcwd(),
    )
    args = parser.parse_args()
    if not check_licenses(args.directory):
        sys.exit(1)


if __name__ == "__main__":
    main()
