#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os.path
import sys
import tempfile


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        help="Write to this file",
        required=True,
    )
    parser.add_argument(
        "--path",
        help="package manifest list as a json file with the names in '.content.names[]'",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        help="A path to write a depfile to",
    )
    args = parser.parse_args()

    out = tempfile.NamedTemporaryFile(
        "w",
        dir=os.path.dirname(args.output),
        # As we'll move this file over the actual output file, we don't want Python to try to
        # automatically delete the file when we're done.
        delete=False,
    )
    try:
        package_names = set()
        with open(args.path, "r") as f:
            package_manifest_list = json.load(f)
            manifest_paths = package_manifest_list["content"].get(
                "manifests", []
            )

        for path in manifest_paths:
            with open(path) as manifest_file:
                manifest = json.load(manifest_file)
                package_names.add(manifest["package"]["name"])

        out_package_names_list = {
            "content": {"names": sorted(package_names)},
            "version": "1",
        }

        out.write(json.dumps(out_package_names_list, indent=2, sort_keys=True))
        out.close()

        os.replace(out.name, args.output)

        if args.depfile:
            os.makedirs(os.path.dirname(args.depfile), exist_ok=True)
            with open(args.depfile, "w") as depfile:
                if manifest_paths:
                    depfile.write(f"{args.output}: \\\n")
                    depfile.write(
                        " \\\n".join([f"  {path}" for path in manifest_paths])
                    )
                else:
                    depfile.write(f"{args.output}: \n")
    finally:
        try:
            os.unlink(out.name)
        except FileNotFoundError:
            pass

    return 0


if __name__ == "__main__":
    sys.exit(main())
