# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities for extracting, creating, and manipulating debug symbols."""

load(
    "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
    _fuchsia_debug_symbols = "fuchsia_debug_symbols",
    _fuchsia_unstripped_binary = "fuchsia_unstripped_binary",
)

fuchsia_debug_symbols = _fuchsia_debug_symbols
fuchsia_unstripped_binary = _fuchsia_unstripped_binary
