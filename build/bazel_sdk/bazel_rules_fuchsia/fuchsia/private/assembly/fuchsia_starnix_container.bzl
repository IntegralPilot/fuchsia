# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for declaring a Fuchsia starnix container."""

load("@fuchsia_rules_common//assembly:providers.bzl", "FuchsiaStarnixContainerInfo")

def _fuchsia_starnix_container_impl(ctx):
    all_files = [ctx.file.system]
    if ctx.attr.vendor:
        all_files.append(ctx.file.vendor)
    if ctx.attr.fstab:
        all_files.append(ctx.file.fstab)
    all_files += ctx.files.ramdisk
    all_files += ctx.files.init

    system_file_overwrite_srcs = []
    system_file_overwrite_dsts = []
    system_file_create_srcs = []
    system_file_create_dsts = []
    system_file_override_deletions = []

    for target in ctx.attr.system_file_overrides:
        info = target[FuchsiaStarnixFileOverrideInfo]
        if info.type == "override":
            system_file_overwrite_srcs.append(info.src)
            system_file_overwrite_dsts.append(info.dst)
            all_files.append(info.src)
        elif info.type == "delete":
            system_file_override_deletions.append(info.dst)
        elif info.type == "create":
            system_file_create_srcs.append(info.src)
            system_file_create_dsts.append(info.dst)
            all_files.append(info.src)

    return [
        DefaultInfo(files = depset(all_files)),
        FuchsiaStarnixContainerInfo(
            name = ctx.attr.package_name if ctx.attr.package_name else ctx.label.name,
            hals = ctx.attr.hals,
            base = ctx.attr.base,
            skip_subpackages = ctx.attr.skip_subpackages,
            system = ctx.file.system.path,
            vendor = ctx.file.vendor.path if ctx.attr.vendor else None,
            ramdisk = [f.path for f in ctx.files.ramdisk],
            fstab = ctx.file.fstab.path if ctx.attr.fstab else None,
            init = [f.path for f in ctx.files.init],
            system_file_overwrite_srcs = system_file_overwrite_srcs,
            system_file_overwrite_dsts = system_file_overwrite_dsts,
            system_file_create_srcs = system_file_create_srcs,
            system_file_create_dsts = system_file_create_dsts,
            system_file_override_deletions = system_file_override_deletions,
        ),
    ]

FuchsiaStarnixFileOverrideInfo = provider(
    doc = "Information about a file override in a starnix container.",
    fields = {
        "type": "Type of operation: 'override', 'delete', 'create'",
        "src": "Source file (label), optional for delete",
        "dst": "Destination path in system image",
    },
)

fuchsia_starnix_container = rule(
    doc = "Declare a starnix container configuration.",
    implementation = _fuchsia_starnix_container_impl,
    attrs = {
        "package_name": attr.string(
            doc = "Name of the starnix container package",
        ),
        "hals": attr.string_list(
            doc = "Package names of HALs to include",
        ),
        "base": attr.string(
            doc = "Name of package containing base resources to include",
            mandatory = True,
        ),
        "skip_subpackages": attr.bool(
            doc = "Whether to skip including HALs as subpackages",
            default = False,
        ),
        "system": attr.label(
            doc = "Path to system image",
            allow_single_file = True,
            mandatory = True,
        ),
        "vendor": attr.label(
            doc = "Path to vendor image",
            allow_single_file = True,
        ),
        "ramdisk": attr.label_list(
            doc = "Paths to ramdisk images",
            allow_files = True,
        ),
        "system_file_overrides": attr.label_list(
            doc = "List of file overrides",
            providers = [FuchsiaStarnixFileOverrideInfo],
        ),
        "fstab": attr.label(
            doc = "Path to fstab",
            allow_single_file = True,
        ),
        "init": attr.label_list(
            doc = "List of paths to extra init scripts",
            allow_files = True,
        ),
    },
)

def _fuchsia_starnix_file_override_impl(ctx):
    return [
        FuchsiaStarnixFileOverrideInfo(
            type = "override",
            src = ctx.file.src,
            dst = ctx.attr.dst,
        ),
    ]

fuchsia_starnix_file_override = rule(
    doc = "Define a file override in a starnix container.",
    implementation = _fuchsia_starnix_file_override_impl,
    attrs = {
        "src": attr.label(
            doc = "Source file to use as override",
            allow_single_file = True,
            mandatory = True,
        ),
        "dst": attr.string(
            doc = "Destination path in system image",
            mandatory = True,
        ),
    },
)

def _fuchsia_starnix_file_delete_impl(ctx):
    return [
        FuchsiaStarnixFileOverrideInfo(
            type = "delete",
            src = None,
            dst = ctx.attr.dst,
        ),
    ]

fuchsia_starnix_file_delete = rule(
    doc = "Define a file deletion in a starnix container.",
    implementation = _fuchsia_starnix_file_delete_impl,
    attrs = {
        "dst": attr.string(
            doc = "Path in system image to delete",
            mandatory = True,
        ),
    },
)

def _fuchsia_starnix_file_create_impl(ctx):
    return [
        FuchsiaStarnixFileOverrideInfo(
            type = "create",
            src = ctx.file.src,
            dst = ctx.attr.dst,
        ),
    ]

fuchsia_starnix_file_create = rule(
    doc = "Define a file creation in a starnix container.",
    implementation = _fuchsia_starnix_file_create_impl,
    attrs = {
        "src": attr.label(
            doc = "Source file to use for creation",
            allow_single_file = True,
            mandatory = True,
        ),
        "dst": attr.string(
            doc = "Destination path in system image",
            mandatory = True,
        ),
    },
)
