# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
A drop-in replacement for cc_library that is always marked
`target_compatible_with` Fuchsia and by default tagged as manual.

This should be used whenever a cc_library would otherwise be fuchsia-specific
and otherwise incompatible with non-fuchsia platforms.
"""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")

def fuchsia_cc_library(additional_linker_inputs = [], tags = ["manual"], **kwargs):
    cc_library(
        additional_linker_inputs = additional_linker_inputs + COMPATIBILITY.FUCHSIA_DEPS,
        tags = tags,
        **kwargs
    )
