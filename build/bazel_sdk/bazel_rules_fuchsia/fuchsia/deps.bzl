# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines rules for use in WORKSPACE files.

See also:
 - @fuchsia_sdk//fuchsia:clang.bzl
 - @fuchsia_sdk//fuchsia:products.bzl
"""

load(
    "//fuchsia/workspace:fuchsia_sdk_repository.bzl",
    _fuchsia_sdk_ext = "fuchsia_sdk_ext",
    _fuchsia_sdk_repository = "fuchsia_sdk_repository",
)
load(
    "//fuchsia/workspace:python_runtime_repository.bzl",
    _python_runtime_repository = "python_runtime_repository",
)
load(
    "//fuchsia/workspace:register_fuchsia_sdk_toolchain.bzl",
    _register_fuchsia_sdk_toolchain = "register_fuchsia_sdk_toolchain",
)
load(
    "//fuchsia/workspace:rules_fuchsia_deps.bzl",
    _rules_fuchsia_deps = "rules_fuchsia_deps",
)

# See corresponding `.bzl` files in fuchsia/private for documentation.
fuchsia_sdk_ext = _fuchsia_sdk_ext
fuchsia_sdk_repository = _fuchsia_sdk_repository
rules_fuchsia_deps = _rules_fuchsia_deps
python_runtime_repository = _python_runtime_repository
register_fuchsia_sdk_toolchain = _register_fuchsia_sdk_toolchain
