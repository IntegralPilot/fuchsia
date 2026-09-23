#!/usr/bin/env python3
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
from pathlib import Path


def parse_args():
    """Parses arguments."""
    parser = argparse.ArgumentParser()

    def path_arg():
        def arg(path):
            path = Path(path)
            if path.is_dir():
                parser.error(f'Path "{path}" is a directory')
            return path

        return arg

    parser.add_argument(
        "--config-file",
        type=path_arg(),
        help="A path to the configuration file.",
        required=True,
    )
    parser.add_argument(
        "--sdk-workspace-name",
        type=str,
        help="The name of the fuchsia_sdk bazel workspace",
        required=False,
        default="fuchsia_sdk",
    )

    return parser.parse_args()


def main():
    args = parse_args()
    try:
        with open(args.config_file, "r") as f:
            config = json.load(f)
    except:
        config = {}

    with open(args.config_file, "w+") as f:
        sdk = config.get("sdk", {})
        sdk["root"] = f"$BUILD_DIR/external/{args.sdk_workspace_name}"

        config["sdk"] = sdk
        json.dump(config, f, indent=4)


if __name__ == "__main__":
    main()
