# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
""" This script generates the component manifest used by the validation-client component."""

import argparse
import sys

from assembly import PackageManifest
from serialization import json_load


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_file")
    parser.add_argument("package_manifest_file")

    args = parser.parse_args()

    with open(args.package_manifest_file, "r") as f:
        manifest = json_load(PackageManifest, f)
        try:
            test_hash = next(
                b.merkle for b in manifest.blobs if b.path == "meta/"
            )
        except StopIteration:
            raise ValueError(
                "manifest %s does not contain an entry for 'meta/'"
                % args.package_manifest_file
            )

    open(args.output_file, "w").write(
        """{ children: [
            {
                name: "validation-server",
                url: "fuchsia-pkg://fuchsia.com/validation-server-pkg?hash=%s#meta/validation-server.cm",
            },
        ],
        expose: [
            {
                directory: "validation_server_pkg",
                from: "#validation-server",
            },
        ],
        offer: [
            {
                dictionary: "diagnostics",
                from: "parent",
                to: "#validation-server",
            },
        ],
    }
        """
        % test_hash
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
