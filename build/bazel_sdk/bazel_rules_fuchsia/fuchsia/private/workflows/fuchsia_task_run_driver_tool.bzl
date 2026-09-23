# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs driver tools delivered within a package."""

load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load(":fuchsia_task_ffx.bzl", "ffx_task_rule")
load(":providers.bzl", "FuchsiaDriverToolInfo")

def _fuchsia_task_run_driver_tool_impl(ctx, make_ffx_task):
    repo = ctx.attr.repository
    package = ctx.attr.package[FuchsiaPackageInfo].package_name
    tool_binary = ctx.attr.tool[FuchsiaDriverToolInfo].tool_path
    url = "fuchsia-pkg://%s/%s#%s" % (repo, package, tool_binary)
    return make_ffx_task(prepend_args = [
        "driver",
        "run-tool",
        url,
    ])

(
    _fuchsia_task_run_driver_tool,
    _fuchsia_task_run_driver_tool_for_test,
    fuchsia_task_run_driver_tool,
) = ffx_task_rule(
    implementation = _fuchsia_task_run_driver_tool_impl,
    attrs = {
        "repository": attr.string(
            doc = "The repository that has the published package.",
            mandatory = True,
        ),
        "package": attr.label(
            doc = "The package containing the driver tool.",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
        "tool": attr.label(
            doc = "The driver tool to run.",
            providers = [FuchsiaDriverToolInfo],
            mandatory = True,
        ),
    },
)
