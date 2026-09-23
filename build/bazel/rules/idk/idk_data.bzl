# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK data atoms."""

load(
    "//build/bazel/rules/idk/private:idk_data.bzl",
    _idk_data = "idk_data",
)

idk_data = _idk_data
