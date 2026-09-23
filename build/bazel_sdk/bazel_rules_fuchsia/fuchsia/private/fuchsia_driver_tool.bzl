# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load(":providers.bzl", "FuchsiaDriverToolInfo", "FuchsiaUnstrippedBinaryInfo")

def _fuchsia_driver_tool_impl(ctx):
    return [
        FuchsiaDriverToolInfo(
            tool_path = ctx.attr.binary[FuchsiaUnstrippedBinaryInfo].dest,
        ),
    ]

fuchsia_driver_tool = rule(
    doc = """Creates a tool which can be used with ffx driver run-tool.

    This rule will create a tool which can be used in the development of a driver.
    The rule takes a binary which is what will be executed when it runs. When the
    tool is added to a package it can be executed via `bazel run my_pkg.my_tool`.
    This will create a package server, publish the package and call `ffx driver run-tool`

    ```
    fuchsia_cc_binary(
        name = "bin",
        srcs = [ "main.cc" ],
    )

    fuchsia_driver_tool(
        name = "my_tool",
        binary = ":bin",
    )

    fuchsia_package(
        name = "pkg",
        tools = [ ":my_tool" ]
    )

    $ bazel run //pkg.my_tool -- --arg1 foo --arg2 bar
    """,
    implementation = _fuchsia_driver_tool_impl,
    attrs = {
        "binary": attr.label(
            doc = "The tool's fuchsia_cc_binary() target.",
            mandatory = True,
            providers = [[FuchsiaUnstrippedBinaryInfo]],
        ),
    },
)
