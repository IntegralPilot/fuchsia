# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Arguments for creating Zedboot recovery images. Intentionally kept in
fuchsia.git so they can be changed together with their GN counterparts.

These are extracted into a loadable .bzl file for sharing between repos.
"""

load(
    "@rules_fuchsia//fuchsia:assembly.bzl",
    "BUILD_TYPES",
)

ZEDBOOT_IMAGE_ARGS = {
    "platform_artifacts": "//build/bazel/assembly/assembly_input_bundles:platform_bringup",
}

ZEDBOOT_PLATFORM_BASE = {
    "build_type": BUILD_TYPES.ENG,
    "feature_set_level": "bootstrap",
    "development_support": {
        "include_netsvc": True,
        "enable_netsvc_netboot": True,
        "tools": {
            "storage": {
                "enable_partitioning_tools": True,
            },
        },
    },
    "graphics": {
        "virtual_console": {
            "color_scheme": "special",
            "dpi": [160, 240, 360, 480],
        },
    },
}
