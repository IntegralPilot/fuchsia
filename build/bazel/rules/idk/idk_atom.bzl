# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK atoms."""

load(
    "//build/bazel/rules/idk/private:idk_atom.bzl",
    _idk_noop_atom = "idk_noop_atom",
)

idk_noop_atom = _idk_noop_atom
