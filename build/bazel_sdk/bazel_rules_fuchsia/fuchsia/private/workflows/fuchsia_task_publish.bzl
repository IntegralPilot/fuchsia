# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Publishes packages as a workflow task."""

load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load("//fuchsia/private:providers.bzl", "FuchsiaPackageGroupInfo")
load(":fuchsia_task.bzl", "fuchsia_task_rule")

def _fuchsia_task_publish_impl(ctx, make_fuchsia_task):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    far_files = [
        pkg.far_file
        for dep in ctx.attr.packages
        for pkg in (dep[FuchsiaPackageGroupInfo].packages if FuchsiaPackageGroupInfo in dep else [dep[FuchsiaPackageInfo]])
    ]

    repo_name_args = [
        "--repo_name",
        ctx.attr.package_repository_name,
    ] if ctx.attr.package_repository_name else []
    return make_fuchsia_task(
        task_runner = ctx.attr._publish_packages_tool,

        # NOTE: make_fuchsia_task() collects all File instances in prepend_args
        # as runfiles, and the `ffx publish` command does not depend on other
        # host tools at run time, so a `runfiles` attribute is not necessary.
        prepend_args = [
            "--ffx",
            sdk.ffx,
            "--package",
        ] + far_files + repo_name_args,
    )

(
    _fuchsia_task_publish,
    _fuchsia_task_publish_for_test,
    fuchsia_task_publish,
) = fuchsia_task_rule(
    implementation = _fuchsia_task_publish_impl,
    doc = """A workflow task that publishes multiple fuchsia packages.""",
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "packages": attr.label_list(
            doc = "The packages to publish.",
            providers = [[FuchsiaPackageInfo], [FuchsiaPackageGroupInfo]],
        ),
        "package_repository_name": attr.string(
            doc = "Optionally specify the repository name to publish these packages to.",
        ),
        "_publish_packages_tool": attr.label(
            doc = "The publish_packages tool.",
            default = "//fuchsia/tools:publish_packages",
            executable = True,
            cfg = "exec",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
