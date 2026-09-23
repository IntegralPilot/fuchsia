#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Takes the output package manifests from a GN metadata walk, and parses
it into a PackageManifestList JSON format. For use in the template
//build/packages:generate_package_metadata"""

import argparse
import json
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="dump of package manifests")
    parser.add_argument("--output", help="Write to this file")
    args = parser.parse_args()

    out_package_manifest_list = {"content": {"manifests": []}, "version": "1"}
    with open(args.input, "r") as f:
        for line in f.readlines():
            out_package_manifest_list["content"]["manifests"].append(
                line.strip()
            )

    with open(args.output, "w") as out:
        json.dump(out_package_manifest_list, out, indent=2, sort_keys=True)

    return 0


if __name__ == "__main__":
    sys.exit(main())
