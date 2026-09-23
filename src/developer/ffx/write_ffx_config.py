# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import argparse
import json
import os


def replace_placeholder(config_data, placeholder, val):
    if isinstance(config_data, dict):
        for k, v in config_data.items():
            if isinstance(v, str) and placeholder in v:
                config_data[k] = v.replace(placeholder, val)
            else:
                config_data[k] = replace_placeholder(v, placeholder, val)
    elif isinstance(config_data, list):
        new_v = []
        for item in config_data:
            if isinstance(item, str) and placeholder in item:
                new_v.append(item.replace(placeholder, val))
            else:
                new_v.append(replace_placeholder(item, placeholder, val))
        config_data = new_v
    return config_data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--dollar-placeholder", help="Placeholder string to replace with '$'"
    )
    parser.add_argument(
        "--data",
        help="The data collected from metadata to write into the config",
    )
    parser.add_argument(
        "--build-dir", help="The build directory to replace with $BUILD_DIR"
    )
    parser.add_argument("--output", help="The path to write the config file")
    args = parser.parse_args()

    config_items = json.load(open(args.data))
    # merge the array of data items into a since dictionary.
    config_data = {}
    # Hack in some variables

    for o in config_items:
        for k, v in o.items():
            config_data[k] = v
    if "ffx" in config_data:
        ffx_data = config_data["ffx"]
    else:
        ffx_data = {}
        config_data["ffx"] = ffx_data

    if "subtool-search-paths" not in ffx_data:
        ffx_data["subtool-search-paths"] = [f"{args.build_dir}/host-tools"]

    if "subtool-manifest" not in ffx_data:
        ffx_data["subtool-manifest"] = f"{args.build_dir}/ffx_tools.json"
    # Always replace build dir. GN always writes "$BUILD_DIR" as "\$BUILD_DIR" so use python.
    replace_placeholder(config_data, args.dollar_placeholder, "$")
    replace_placeholder(
        config_data, os.path.abspath(args.build_dir), "$BUILD_DIR"
    )

    with open(args.output, "w") as f:
        json.dump(config_data, f, indent=2)


if __name__ == "__main__":
    main()
