# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines the ffx_tool macro for building ffx subtools."""

load("//build/bazel/rules/host:defs.bzl", "rustc_binary_host_tool")
load("//src/developer/ffx/lib/version/build:ffx_apply_version.bzl", "ffx_apply_version")

def _ffx_tool_impl(name, **kwargs):
    rustc_binary_host_tool(
        name = name,
        **kwargs
    )

    ffx_apply_version(
        name = name + "_versioned",
        unversioned_binary = name,
    )

ffx_tool = macro(
    doc = """Defines an ffx-compatible subtool.

    Wraps `rustc_binary_host_tool` to create an unversioned binary, applies version info
    to create the main versioned binary.

    Two subtargets are created by this macro:

      - name: The unversioned binary.
      - name + "_versioned": The versioned binary.

    An unversioned binary is created to avoid build cache invalidations. This makes incremental
    builds much faster for users who don't need version information in ffx binaries.

    A versioned binary is created for official builds, SDK distributions, and runtime compatibility
    checks. The version metadata is required by ffx and the daemon to verify compatibility between
    the subtools and the target device.

    TODO(https://fxbug.dev/512640761): Match the support implemented in ffx_tool.gni
    """,
    implementation = _ffx_tool_impl,
    inherit_attrs = rustc_binary_host_tool,
    attrs = {},
)
