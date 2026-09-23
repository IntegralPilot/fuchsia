# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for defining a Fuchsia devicetree toolchain."""

def _fuchsia_devicetree_toolchain_info_impl(ctx):
    return [platform_common.ToolchainInfo(
        name = ctx.label.name,
        dtc = ctx.executable.dtc,
    )]

fuchsia_devicetree_toolchain_info = rule(
    implementation = _fuchsia_devicetree_toolchain_info_impl,
    doc = """
Fuchsia devicetree toolchain info rule, to be passed to the native `toolchain`
rule. It provides information about dtc tools.
""",
    attrs = {
        "dtc": attr.label(
            doc = "dtc tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
    },
    provides = [platform_common.ToolchainInfo],
)
