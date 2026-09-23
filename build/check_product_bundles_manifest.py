#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys

USAGE = "This scripts asserts product bundle names are unique."


def main():
    parser = argparse.ArgumentParser(usage=USAGE)
    parser.add_argument(
        "--product-bundle-json",
        help="Path to the product_bundles.json build API module",
        required=True,
    )
    parser.add_argument(
        "--expect-product-bundle-name",
        help="A product bundle name that MUST exist in the manifest",
    )
    parser.add_argument(
        "--stamp",
        help="Path to the stamp file to emit",
        required=True,
    )

    args = parser.parse_args()
    with open(args.product_bundle_json) as f:
        product_bundles = list(map(lambda entry: entry["name"], json.load(f)))

    if args.expect_product_bundle_name:
        if args.expect_product_bundle_name not in product_bundles:
            print(
                "\nFailed to find required product bundle with name: %s.\n"
                % args.expect_product_bundle_name,
                file=sys.stderr,
            )
            return 1

    counts = {}
    for pb in product_bundles:
        counts[pb] = counts.get(pb, 0) + 1
    duplicates = [pb for (pb, count) in counts.items() if count > 1]

    if len(duplicates) > 0:
        print(
            "Found product bundles in %s with duplicate names: %s."
            % (args.product_bundle_json, ", ".join(duplicates)),
            file=sys.stderr,
        )
        return 1

    with open(args.stamp, "w") as f:
        os.utime(f.name, None)

    return 0


if __name__ == "__main__":
    sys.exit(main())
