#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Gets relevant supported and in development Fuchsia platform versions from main config file.
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def main() -> int:
    parser = argparse.ArgumentParser(
        "Processes version_history.json to return list of supported and in-development API levels."
    )
    parser.add_argument(
        "--version-history-path",
        type=Path,
        required=True,
        help="Path to the version history JSON file",
    )

    args = parser.parse_args()
    print(json.dumps(get_gn_variables(args.version_history_path)))
    return 0


def get_gn_variables(version_history_path: Path) -> dict[str, Any]:
    """Reads from version_history.json to generate some data to expose to the GN
    build graph."""

    try:
        with open(version_history_path) as file:
            data = json.load(file)
    except FileNotFoundError:
        print(
            """error: Unable to open '{path}'. Did you run this script from the root of the source tree?""".format(
                path=version_history_path
            ),
            file=sys.stderr,
        )
        raise

    api_levels = data["data"]["api_levels"]

    all_numbered_api_levels = [int(level) for level in api_levels]

    retired_api_levels: list[int] = [
        int(level)
        for level in api_levels
        if api_levels[level]["phase"] == "retired"
    ]
    sunset_api_levels: list[int] = [
        int(level)
        for level in api_levels
        if api_levels[level]["phase"] == "sunset"
    ]
    supported_api_levels: list[int] = [
        int(level)
        for level in api_levels
        if api_levels[level]["phase"] == "supported"
    ]

    assert len(api_levels) == (
        len(retired_api_levels)
        + len(sunset_api_levels)
        + len(supported_api_levels)
    ), '"api_levels" contains a level with an unexpected "phase".'

    # Special API levels are added below.
    runtime_supported_api_levels = sunset_api_levels + supported_api_levels
    idk_buildable_api_levels = supported_api_levels.copy()

    # Explicitly add concrete special API levels.
    # "HEAD" is not supported in the IDK - see https://fxbug.dev/334936990.
    runtime_supported_api_levels += ["NEXT", "HEAD"]
    idk_buildable_api_levels += ["NEXT"]

    # Validate the JSON data, including the specific levels and their phase.
    expected_special_levels = ["NEXT", "HEAD", "PLATFORM"]
    special_api_levels = data["data"]["special_api_levels"]
    assert len(special_api_levels) == len(expected_special_levels)
    for level in special_api_levels:
        expected_level = expected_special_levels.pop(0)
        assert (
            level == expected_level
        ), f'Special API level "{level}" is in the position expected to contain "{expected_level}".'

    return {
        # All numbered API levels in the JSON file.
        # All are frozen or previously frozen. None are in-development.
        "all_numbered_api_levels": all_numbered_api_levels,
        #
        # API levels that the IDK supports targeting (by default).
        # Do not use this directly. Instead, use GN arg `idk_buildable_api_levels`.
        "default_idk_buildable_api_levels": idk_buildable_api_levels,
        #
        # LINT.IfChange
        # API levels that the platform build supports at runtime.
        "runtime_supported_api_levels": runtime_supported_api_levels,
        # LINT.ThenChange(//build/bazel/BUILD.gn:get_platform_version)
        #
        # API levels whose contents should not change anymore.
        "frozen_api_levels": sunset_api_levels + supported_api_levels,
    }


if __name__ == "__main__":
    sys.exit(main())
