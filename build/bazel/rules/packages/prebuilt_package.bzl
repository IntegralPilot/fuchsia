# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for prebuilt packages for platform testing."""

load("@fuchsia_rules_common//packages:prebuilt_package.bzl", "unpack_prebuilt_package_impl")

def _prebuilt_package_impl(ctx):
    return unpack_prebuilt_package_impl(
        ctx,
        package_tool = ctx.executable._package_tool,
    )

prebuilt_package = rule(
    implementation = _prebuilt_package_impl,
    attrs = {
        "archive": attr.label(
            doc = "The fuchsia archive (typically a .far file).",
            allow_single_file = True,
            mandatory = True,
        ),
        "_package_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/src/sys/pkg/bin/package-tool",
            executable = True,
            cfg = "exec",
        ),
        "_rebase_package_manifest": attr.label(
            default = "@fuchsia_rules_common//packages:rebase_package_manifest",
            executable = True,
            cfg = "exec",
        ),
    },
)
