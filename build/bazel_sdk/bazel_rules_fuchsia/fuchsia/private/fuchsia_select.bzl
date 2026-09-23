# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helpers for using select() within Fuchsia rules."""

_ERROR = """
****************************************************************************
ERROR: You have to specify a config in order to build Fuchsia.
For example:
    bazel build --config=fuchsia_x64 ...
    bazel build --config=fuchsia_arm64 ...
****************************************************************************
"""

_VARIANT_ERROR = """
****************************************************************************
ERROR: Current configuration does not match with any condition.

Conditions checked:
{}

Check the cpu and API level in your configuration. You can also temporarily overwrite the API level you are targeting.
For example:
    bazel build --@fuchsia_sdk//flags:fuchsia_api_level=HEAD ...
    bazel build --@fuchsia_sdk//flags:fuchsia_api_level=23 ...
****************************************************************************
"""

def fuchsia_select(configs):
    """select() variant that prints a meaningful error.

    Args:
        configs: A dict of config name-value pairs.

    Returns:
        Selected attribute value depending on the config.
    """
    return select(configs, no_match_error = _ERROR)

def variant_select(configs):
    """select() variant that prints a meaningful error.

    Args:
        configs: A dict of config name-value pairs.

    Returns:
        Selected attribute value depending on the config.
    """
    error = _VARIANT_ERROR.format(json.encode_indent(configs.keys()))
    return select(configs, no_match_error = error)
