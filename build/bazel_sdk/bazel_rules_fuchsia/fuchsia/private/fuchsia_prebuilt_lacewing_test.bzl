# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines the fuchsia_prebuilt_lacewing_test build rule."""

load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":utils.bzl", "wrap_executable")

def _fuchsia_prebuilt_lacewing_test_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    py_toolchain = ctx.toolchains["@rules_python//python:toolchain_type"]
    if not py_toolchain.py3_runtime:
        fail("A Bazel python3 runtime is required, and none was configured!")

    executable, runfiles = wrap_executable(
        ctx,
        py_toolchain.py3_runtime.interpreter,
        ctx.attr._run_lacewing_test_tool,
        "--test-binary",
        ctx.attr.test_binary,
        "--ffx",
        sdk.ffx,
        "--name",
        ctx.label.name,
    )
    return [
        DefaultInfo(
            executable = executable,
            runfiles = ctx.runfiles(
                ctx.files.data,
                transitive_files = py_toolchain.py3_runtime.files,
            ).merge(runfiles),
        ),
    ]

fuchsia_prebuilt_lacewing_test = rule(
    doc = "Defines a prebuilt lacewing test.",
    implementation = _fuchsia_prebuilt_lacewing_test_impl,
    toolchains = [
        FUCHSIA_TOOLCHAIN_DEFINITION,
        "@rules_python//python:toolchain_type",
    ],
    attrs = {
        "test_binary": attr.label(
            doc = "A path to the prebuilt lacewing test's binary.",
            allow_single_file = True,
            mandatory = True,
        ),
        "data": attr.label_list(
            doc = "Files to make available when running the lacewing test.",
            allow_files = True,
        ),
        "_run_lacewing_test_tool": attr.label(
            doc = "The tool used to run the lacewing test.",
            default = "//fuchsia/tools:run_lacewing_test",
            executable = True,
            cfg = "exec",
        ),
    },
    test = True,
)
