# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK transitions."""

load(
    "//build/bazel/rules/idk/private:idk_transitions.bzl",
    _build_in_all_idk_api_level_and_cpu_combinations = "build_in_all_idk_api_level_and_cpu_combinations",
)

build_in_all_idk_api_level_and_cpu_combinations = _build_in_all_idk_api_level_and_cpu_combinations
