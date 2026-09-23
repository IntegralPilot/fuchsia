# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")

# buildifier: disable=module-docstring
# buildifier: disable=function-docstring
def _sdk_host_tool_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    file = getattr(sdk, ctx.label.name)
    exe = ctx.actions.declare_file(ctx.label.name + "_wrap.sh")
    ctx.actions.write(exe, """
    #!/bin/bash
    $0.runfiles/{}/{} "$@"
    """.format(ctx.workspace_name, file.short_path), is_executable = True)

    return [DefaultInfo(
        executable = exe,
        runfiles = ctx.runfiles([file] + ctx.files._sdk_runfiles),
    )]

sdk_host_tool = rule(
    implementation = _sdk_host_tool_impl,
    doc = """
    A rule which can wrap tools found in the fuchsia sdk toolchain.

    The rule will look for the name of the tool to be invoked based
    on the name of the target. These targets can then be executed
    directly or the user can use the run_sdk_tool shell script.

    The following rule will wrap the ffx binary.
    ```
    sdk_host_tool(name = "ffx")
    ```
    """,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    executable = True,
    attrs = {
        "_sdk_runfiles": attr.label(
            doc = "Allows the entire SDK to be available for `bazel run` SDK tool invocations.",
            allow_files = True,
            default = "@fuchsia_sdk//:all_files",
        ),
    },
)
