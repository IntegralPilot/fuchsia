#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Prepare a distribution manifest for the bootfs files

Reads a distribution manifest, adds a "bootfs/" prefix to all meta/something
destinations, and writes a new distribution manifest.

This ensures that the entries are not shoved into a meta.far and lost inside
a fuchsia_package.
"""

import argparse
import json
import os


def main():
    parser = argparse.ArgumentParser(
        description="Prepare a distribution manifest for bootfs files"
    )
    parser.add_argument("--input", type=argparse.FileType("r"), required=True)
    parser.add_argument("--output", type=argparse.FileType("w"), required=True)
    args = parser.parse_args()

    manifest = json.load(args.input)
    for entry in manifest:
        if "destination" in entry and entry["destination"].startswith("meta/"):
            entry["destination"] = os.path.join("bootfs", entry["destination"])
    json.dump(manifest, args.output, indent=4)


if __name__ == "__main__":
    main()
