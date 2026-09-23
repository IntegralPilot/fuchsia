# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK host tools."""

load(
    "//build/bazel/rules/idk/private:idk_host_tool.bzl",
    _idk_cc_binary_host_tool = "idk_cc_binary_host_tool",
    _idk_go_binary_host_tool = "idk_go_binary_host_tool",
    _idk_rustc_binary_host_tool = "idk_rustc_binary_host_tool",
)

idk_cc_binary_host_tool = _idk_cc_binary_host_tool
idk_go_binary_host_tool = _idk_go_binary_host_tool
idk_rustc_binary_host_tool = _idk_rustc_binary_host_tool
