#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys

import json5


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate a component for Starnix container config in a test."
    )
    parser.add_argument("--schema", type=argparse.FileType("r"), required=True)
    parser.add_argument(
        "--overrides", type=argparse.FileType("r"), required=True
    )
    parser.add_argument("--output", type=argparse.FileType("w"), required=True)
    args = parser.parse_args()

    with args.schema as f:
        schema = json5.load(f)
    with args.overrides as f:
        overrides = json.load(f)

    known_keys = []
    capabilities = []
    expose = []
    for cap_schema in schema["use"]:
        key = cap_schema["key"]
        known_keys.append(key)
        if key in overrides:
            value = overrides[key]
            # Remove known config keys from the overrides so after this only unknown keys remain.
            del overrides[key]
        else:
            value = cap_schema["default"]
        new_cap = {
            "config": cap_schema["config"],
            "type": cap_schema["type"],
            "value": value,
        }

        for k in ["element", "max_count", "max_size"]:
            if k in cap_schema:
                new_cap[k] = cap_schema[k]
        capabilities.append(new_cap)
        expose.append(
            {
                "config": cap_schema["config"],
                "from": "self",
            }
        )

    if len(overrides) > 0:
        known_keys = sorted(known_keys)
        print(f"\nERROR: Unknown config keys:")
        for key in overrides:
            print(f"\t* {key}")
        print(f"\nValid keys for container config overrides:")
        for key in known_keys:
            print(f"\t* {key}")
        return 1

    manifest = {
        "capabilities": capabilities,
        "expose": expose,
    }
    with args.output as f:
        f.write(json.dumps(manifest))

    return 0


if __name__ == "__main__":
    sys.exit(main())
