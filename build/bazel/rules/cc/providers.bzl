# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines providers related to C++ libraries."""

visibility([
    "//build/bazel/rules/idk/...",
    "//build/bazel/bazel_idk/tests/...",
])

# LINT.IfChange(prebuilt_library_info)
PrebuiltLibraryInfo = provider(
    doc = "Provides various files related to a built library. " +
          "Not all library types support all fields.",
    fields = {
        "type": "The type of library (e.g. 'shared', 'static').",
        "debug": "The unstripped shared library file with debug information.",
        "stripped": "The stripped shared library file.",
        "link_lib": "The link stub for the library.",
        "unstripped_ifs_file": "The unstripped IFS file for the library.",
        "stripped_ifs_file": "The IFS file stripped of extra information such as undefined symbols and CPU architecture.",
    },
)
# LINT.ThenChange(//build/bazel/rules/cc/shared_library.bzl:prebuilt_library_info)
