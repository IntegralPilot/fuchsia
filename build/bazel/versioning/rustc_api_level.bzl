# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Provides functions to generate Rust configuration flags based on the target Fuchsia API level."""

# NOTE: This file has public visibility because it is loaded by
# `@fuchsia_prebuilt_rust`, and Bazel load visibility list does not support
# entries starting with `@`. This file is intentionally separated from
# api_level.bzl from the same directory to avoid polluting the load visibility
# list there.

load("@//build/bazel/versioning:api_level.bzl", "get_integer_for_api_level")
load("@fuchsia_build_info//:args.bzl", "all_numbered_api_levels", "idk_buildable_api_levels")

def get_rustc_api_level_flags():
    """Generates a select() statement for rustc fuchsia_api_level config flags.

    Returns:
        A select() statement mapping API levels to their appropriate flags.
    """
    conditions = {}

    special_levels = ["NEXT", "HEAD", "PLATFORM"]
    all_levels = all_numbered_api_levels + special_levels
    target_levels = list(idk_buildable_api_levels)

    # idk_buildable_api_levels can contain `NEXT`, so dedupe to avoid
    # redundant work below.
    for level in special_levels:
        if level not in target_levels:
            target_levels.append(level)

    for target_level in target_levels:
        target_int = get_integer_for_api_level(target_level)
        flags = []
        for historical_level in all_levels:
            if get_integer_for_api_level(historical_level) <= target_int:
                flags.append('--cfg=fuchsia_api_level_at_least="{}"'.format(historical_level))
            else:
                flags.append('--cfg=fuchsia_api_level_less_than="{}"'.format(historical_level))

        conditions["@//build/bazel/versioning:is_api_level_" + target_level] = flags

    return select(conditions)
