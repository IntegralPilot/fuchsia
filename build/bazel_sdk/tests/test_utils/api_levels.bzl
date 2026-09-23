# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities related to production API levels."""

load("@fuchsia_sdk//:api_version.bzl", "INTERNAL_ONLY_SUPPORTED_API_LEVELS")

def some_valid_numerical_api_level_as_string():
    # The first element is always a numerical API level. This will fail if that changes.
    _valid_numerical_api_level = int(INTERNAL_ONLY_SUPPORTED_API_LEVELS[0].api_level)
    if _valid_numerical_api_level < 10 or _valid_numerical_api_level > 1000:
        fail("First API level '%d' is not in expected range." % _valid_numerical_api_level)

    return str(_valid_numerical_api_level)
