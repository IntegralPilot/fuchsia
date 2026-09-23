# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Clang runtime libraries as Fuchsia package resources."""

load(
    "@fuchsia_rules_common//packages:clang_package_resources.bzl",
    _get_clang_package_resources = "get_clang_package_resources",
)

get_clang_package_resources = _get_clang_package_resources
