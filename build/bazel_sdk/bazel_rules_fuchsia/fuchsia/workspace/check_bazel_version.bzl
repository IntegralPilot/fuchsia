# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Note: This code is heavily inspired by the bazel-skylibs implementation
# for checking the bazel version. We are not using the implementation here to
# avoid pulling in an extra dependency and to add some messaging that is
# specific to the Fuchsia Bazel SDK.

# buildifier: disable=module-docstring
_MIN_VERSION_FAIL_MESSAGE = """
**************************************************************************
The version of bazel does not meet the minimum version requirements.
Please update your version of Bazel or run the bootstrap script again.
Current Version: {bazel_version}
Minimum Version: {min_version}
**************************************************************************
"""

_MAX_VERSION_FAIL_MESSAGE = """
**************************************************************************
The version of bazel does not meet the maximum version requirements.
Please downgrade your version of Bazel or run the bootstrap script again.
Current Version: {bazel_version}
Maximum Version: {max_version}
**************************************************************************
"""

def _version_string_to_tupl(s):
    return tuple([int(n) for n in s.split(".")])

def _get_bazel_version():
    bazel_version = native.bazel_version
    for i in range(len(bazel_version)):
        c = bazel_version[i]
        if not (c.isdigit() or c == "."):
            return bazel_version[:i]
    return bazel_version

def _is_at_least(min, bazel_version):
    if min:
        return _version_string_to_tupl(min) <= _version_string_to_tupl(bazel_version)
    return True

def _is_at_most(max, bazel_version):
    if max:
        return _version_string_to_tupl(max) >= _version_string_to_tupl(bazel_version)
    return True

def assert_bazel_version(min = None, max = None):
    """Performs a check against the current version of Bazel

    Args:
        min: The minimum version allowed
        max: The maximum version allowed
    """
    bazel_version = _get_bazel_version()
    if not bazel_version:
        # assume the user knows what they are doing since they are using an
        # unreleased version of bazel
        return

    if not _is_at_least(min, bazel_version):
        fail(_MIN_VERSION_FAIL_MESSAGE.format(bazel_version = bazel_version, min_version = min))
    if not _is_at_most(max, bazel_version):
        fail(_MAX_VERSION_FAIL_MESSAGE.format(bazel_version = bazel_version, max_version = max))
