# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Build rule for pre-built Fuchsia Packages."""

load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/private/workflows:fuchsia_task_publish.bzl", "fuchsia_task_publish")
load(
    ":providers.bzl",
    "FuchsiaPackageGroupInfo",
)

def _fuchsia_package_group_impl(ctx):
    packages = []
    for dep in ctx.attr.deps:
        if FuchsiaPackageInfo in dep:
            packages.append(dep[FuchsiaPackageInfo])
        elif FuchsiaPackageGroupInfo in dep:
            packages.extend(dep[FuchsiaPackageGroupInfo].packages)
    return [
        DefaultInfo(files = depset(ctx.files.deps)),
        FuchsiaPackageGroupInfo(packages = packages),
    ]

_fuchsia_package_group = rule(
    doc = """
A group of Fuchsia packages, composed of all the packages and groups specified in deps.
""",
    implementation = _fuchsia_package_group_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Fuchsia packages and package groups to include in this group.",
            providers = [
                [FuchsiaPackageInfo],
                [FuchsiaPackageGroupInfo],
            ],
        ),
    },
)

def fuchsia_package_group(*, name, deps, **kwargs):
    _fuchsia_package_group(
        name = name,
        deps = deps,
        **kwargs
    )

    fuchsia_task_publish(
        name = "%s.publish" % name,
        packages = [name],
        **kwargs
    )
