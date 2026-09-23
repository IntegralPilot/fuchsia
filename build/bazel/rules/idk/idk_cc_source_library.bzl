# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public macros for defining C/C++ source libraries in the IDK."""

load(
    "//build/bazel/rules/idk/private:idk_cc_source_library.bzl",
    _idk_cc_source_library = "idk_cc_source_library",
    _idk_cc_source_library_zx = "idk_cc_source_library_zx",
)
load(
    "//build/bazel/rules/idk/private:idk_common.bzl",
    "get_api_file_path",
)

def idk_cc_source_library(
        *,
        idk_name,
        category,
        stable,
        api_file_path = None,
        **kwargs):
    """Defines a C/C++ source library in the IDK.

    See `idk_cc_source_library()` in `private:idk_cc_source_library.bzl`
    for documentation.

    This legacy macro wraps the the private symbolic macro
    `idk_cc_source_library()` to provide a default value for `api_file_path`.
    """
    _idk_cc_source_library(
        idk_name = idk_name,
        category = category,
        stable = stable,
        api_file_path = get_api_file_path(
            idk_name = idk_name,
            stable = stable,
            api_file_path = api_file_path,
        ),
        **kwargs
    )

def idk_cc_source_library_zx(
        *,
        idk_name,
        category,
        stable,
        api_file_path = None,
        **kwargs):
    """Defines a C/C++ source library in the IDK that will be a `zx_library()` in GN.

    See `idk_cc_source_library_zx()` in `private:idk_cc_source_library.bzl`
    for documentation.

    This legacy macro wraps the the private symbolic macro
    `idk_cc_source_library_zx()` to provide a default value for `api_file_path`.
    """
    _idk_cc_source_library_zx(
        idk_name = idk_name,
        category = category,
        stable = stable,
        api_file_path = get_api_file_path(
            idk_name = idk_name,
            stable = stable,
            api_file_path = api_file_path,
        ),
        **kwargs
    )
