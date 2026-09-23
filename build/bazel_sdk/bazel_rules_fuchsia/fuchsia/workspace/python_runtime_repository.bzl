# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines a WORKSPACE rule for fetching python3 from CIPD server based on ensure file."""

load("//fuchsia/workspace:utils.bzl", "fetch_cipd_contents")

_BUILD_CONTENT = """
load("@rules_python//python:defs.bzl", "py_runtime", "py_runtime_pair")

py_runtime(
    name = "py_runtime",
    files = glob(["**/*"], exclude = ["**/* *"]),
    interpreter = "bin/python3",
    python_version = "PY3",
    stub_shebang = \"""#!/bin/bash
"exec" "$0.runfiles/fuchsia_python/bin/python3" "$0" "$@"
\"""
)

py_runtime_pair(
    name = "py_runtime_pair",
    py3_runtime = ":py_runtime",
)

toolchain(
    name = "py3_toolchain",
    toolchain = ":py_runtime_pair",
    toolchain_type = "@rules_python//python:toolchain_type",
)

"""

def _python_runtime_repository_impl(ctx):
    fetch_cipd_contents(ctx, ctx.attr._cipd_bin, ctx.attr._cipd_ensure_file)

    ctx.file(
        "BUILD.bazel",
        _BUILD_CONTENT,
        executable = False,
    )

python_runtime_repository = repository_rule(
    doc = """
Fetch specific version of python3 from CIPD server.
""",
    implementation = _python_runtime_repository_impl,
    attrs = {
        "_cipd_ensure_file": attr.label(
            doc = "A cipd ensure file to use to download the python.",
            default = "//fuchsia/manifests:python.ensure",
        ),
        "_cipd_bin": attr.label(
            doc = "The cipd binary that will be used to download the python",
            default = "@cipd_tool//:cipd",
        ),
    },
)
