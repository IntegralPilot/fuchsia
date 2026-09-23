# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Functions related to the Fuchsia API level."""

# Only toolchains should need to use the API level as an integer.
visibility([
    "//build/bazel/rules/idk/...",
    "//build/bazel/rules/packages/...",
    "//build/bazel/toolchains/...",
])

def get_integer_for_api_level(api_level):
    """Returns the integer reprsentation of the Fuchsia `api_level`.

    This should only be used to pass an integer representation of the current
    target API level to build tools, such as Clang, or to determine whether to
    include a Fuchsia package in the IDK.
    Individual target definitions should not use it.
    """

    # Numerical values associated with special API levels, as defined in
    # https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0246_api_levels_are_32_bits#special_api_levels.
    _FIRST_RESERVED_API_LEVEL = 2147483648  # 0x80000000
    _API_LEVEL_NEXT_AS_INTEGER = 4291821568
    _API_LEVEL_HEAD_AS_INTEGER = 4292870144
    _API_LEVEL_PLATFORM_AS_INTEGER = 4293918720

    if api_level == "NEXT":
        return _API_LEVEL_NEXT_AS_INTEGER
    elif api_level == "HEAD":
        return _API_LEVEL_HEAD_AS_INTEGER
    elif api_level == "PLATFORM":
        return _API_LEVEL_PLATFORM_AS_INTEGER
    else:
        # If the string is not an integer, this will raise a ValueError.
        api_level_integer = int(api_level)

        # `current_build_target_api_level` must be an integer. Ensure it adheres to
        # https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0246_api_levels_are_32_bits#design.
        if api_level_integer < 0:
            fail("Non-special API levels must be a positive integer, not: %s" % api_level_integer)
        if api_level_integer >= _FIRST_RESERVED_API_LEVEL:
            fail("Special API levels should be given by name, not number: %s" % api_level_integer)

        return api_level_integer
