# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for generating an Android Starnix container."""

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")

def _gen_android_starnix_container_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    _hal_files = []
    for hal in ctx.attr.hals:
        _hal_files += hal[FuchsiaPackageInfo].files

    _hal_package_manifests = [
        hal[FuchsiaPackageInfo].package_manifest
        for hal in ctx.attr.hals
    ]
    _package_inputs = [
        ctx.attr.base[FuchsiaPackageInfo].package_manifest,
        ctx.file.system,
    ] + ctx.files.base_files + _hal_files + ctx.attr.base[FuchsiaPackageInfo].files

    _container_manifest = ctx.actions.declare_file(ctx.label.name + "_out/package_manifest.json")

    # Bazel doesn't allow declaring output files under a declared directory, so
    # we can't declare the output directory and the manifest file as output at
    # the same time. To work around that, we explicitly declare siblings of the
    # manifest as outputs. This is coupled with the implementation of
    # gen_android_starnix_container.
    meta_far = ctx.actions.declare_file("meta.far", sibling = _container_manifest)
    _outputs = [
        _container_manifest,
        ctx.actions.declare_directory("meta", sibling = _container_manifest),
        meta_far,
        ctx.actions.declare_directory("odm", sibling = _container_manifest),
        ctx.actions.declare_directory("ramdisk", sibling = _container_manifest),
        ctx.actions.declare_directory("system", sibling = _container_manifest),
    ]

    _args = [
        "--name",
        ctx.attr.package_name if ctx.attr.package_name else ctx.label.name,
        "--outdir",
        _container_manifest.dirname,
        "--base",
        ctx.attr.base[FuchsiaPackageInfo].package_manifest.path,
        "--system",
        ctx.file.system.path,
    ]
    for hal in _hal_package_manifests:
        _args += [
            "--hal",
            hal.path,
        ]

    if ctx.file.vendor:
        _package_inputs.append(ctx.file.vendor)
        _args += [
            "--vendor",
            ctx.file.vendor.path,
        ]
        _outputs.append(ctx.actions.declare_directory("vendor", sibling = _container_manifest))

    if ctx.attr.skip_subpackages:
        _args.append("--skip-subpackages")

    _ramdisks = []
    if ctx.file.ramdisk:
        _ramdisks.append(ctx.file.ramdisk)
    if ctx.files.ramdisks:
        _ramdisks.extend(ctx.files.ramdisks)
    for ramdisk in _ramdisks:
        _package_inputs.append(ramdisk)
        _args += [
            "--ramdisk",
            ramdisk.path,
        ]

    ctx.actions.run(
        executable = sdk.gen_android_starnix_container,
        arguments = _args,
        inputs = _package_inputs,
        outputs = _outputs,
        mnemonic = "GenAndroidStarnixContainer",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset(direct = _outputs)),
        FuchsiaPackageInfo(
            package_manifest = _container_manifest,
            files = _package_inputs + _outputs,
            meta_far = meta_far,
            package_resources = [],
        ),
    ]

fuchsia_gen_android_starnix_container = rule(
    doc = "Construct a starnix container that can include an Android system and HALs.",
    implementation = _gen_android_starnix_container_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "base": attr.label(
            doc = "Path to package containing base resources to include",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
        "base_files": attr.label_list(
            doc = "Files referenced by base package manifest",
            allow_files = True,
        ),
        "package_name": attr.string(
            doc = "Name of the starnix container package",
        ),
        "system": attr.label(
            doc = "The Android system image",
            allow_single_file = True,
            mandatory = True,
        ),
        "vendor": attr.label(
            doc = "The Android vendor image",
            allow_single_file = True,
        ),
        "ramdisk": attr.label(
            doc = "The ramdisk image",
            allow_single_file = True,
        ),
        "ramdisks": attr.label_list(
            doc = "The set of ramdisk images",
            allow_files = True,
        ),
        "hals": attr.label_list(
            doc = "HALs to include in this container. List of fuchsia_prebuilt_package labels or far files",
            allow_files = True,
        ),
        "skip_subpackages": attr.bool(
            doc = "Skip including HALs as subpackages.",
        ),
    },
)
