# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Clang-related provider definitions."""

ClangInfo = provider(
    doc = "Information about a given Clang prebuilt toolchain installation",
    fields = {
        "short_version": "Clang short version, as a string.",
        "long_version": "Clang long version, as a string.",
        "builtin_include_paths": "Clang built-in include paths.",
        "fuchsia_host_arch": "Host cpu name, using Fuchsia conventions.",
        "fuchsia_host_os": "Host os name using Fuchsia conventions.",
    },
)

FuchsiaApiLevelInfo = provider(
    doc = "Exposes the Fuchsia API level as an integer. " +
          "Used to abstract the mechanism used to obtain the API level " +
          "in code used by the SDK and platform.",
    fields = {
        "level": "The integer representation of the target API level.",
    },
)
