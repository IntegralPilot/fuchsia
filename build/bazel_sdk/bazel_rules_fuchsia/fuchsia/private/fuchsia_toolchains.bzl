# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" Helper functions for generating fuchsia sdk toolchains. """

def get_fuchsia_sdk_toolchain(ctx):
    """A helper function which makes it easier to get the fuchsia toolchain.

    This method makes it so you do not need to rely on hardcoded values for the
    toolchain.

    Args:
        ctx: The rule context.

    Returns:
        The fuchsia sdk tool chain.
    """
    sdk = ctx.toolchains["@rules_fuchsia//fuchsia/toolchains:sdk"]

    if not sdk:
        fail("No fuchsia toolchain registered. Please call register_fuchsia_sdk_toolchain in your WORKSPACE file.")
    return sdk

# These toolchain definitions should be used in conjunction with get_fuchsia_sdk_toolchain
# when working with fuchsia toolchains.
FUCHSIA_TOOLCHAIN_DEFINITION = "@rules_fuchsia//fuchsia/toolchains:sdk"
