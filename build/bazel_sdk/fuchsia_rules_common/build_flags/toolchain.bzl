# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Implementation of the build_flags_toolchain_instance() rule."""

load(":providers.bzl", "BuildFlagsInfo", "DefaultBuildFlagsSetInfo")

#############################################################################
#############################################################################
#####
#####    build_flags_toolchain_instance()
#####
#####    This is a Bazel toolchain type to provide different lists of
#####    default build_flags() labels, for different types of targets.
#####
#####

def _build_flags_toolchain_instance_impl(ctx):
    return [
        platform_common.ToolchainInfo(
            default_flags = DefaultBuildFlagsSetInfo(
                cxx_common_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.cxx_common_build_flags
                ],
                cxx_shared_library_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.cxx_shared_library_build_flags
                ],
                cxx_executable_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.cxx_executable_build_flags
                ],
                rust_common_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.rust_common_build_flags
                ],
                rust_shared_library_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.rust_shared_library_build_flags
                ],
                rust_executable_infos = [
                    target[BuildFlagsInfo]
                    for target in ctx.attr.rust_executable_build_flags
                ],
            ),
        ),
    ]

build_flags_toolchain_instance = rule(
    implementation = _build_flags_toolchain_instance_impl,
    doc = "A toolchain() instance target which provides lists of default build flags by target type.",
    attrs = {
        "cxx_common_build_flags": attr.label_list(
            doc = "The build_flags() targets to use by default for all C/C++ targets.",
            providers = [BuildFlagsInfo],
        ),
        "cxx_shared_library_build_flags": attr.label_list(
            doc = "Extra build_flags() targets for C/C++ shared library targets.",
            providers = [BuildFlagsInfo],
        ),
        "cxx_executable_build_flags": attr.label_list(
            doc = "Extra build_flags() targets for C/C++ executable targets.",
            providers = [BuildFlagsInfo],
        ),
        "rust_common_build_flags": attr.label_list(
            doc = "The build_flags() targets to use by default for all Rust targets.",
            providers = [BuildFlagsInfo],
        ),
        "rust_shared_library_build_flags": attr.label_list(
            doc = "Extra build_flags() targets for Rust shared library targets.",
            providers = [BuildFlagsInfo],
        ),
        "rust_executable_build_flags": attr.label_list(
            doc = "Extra build_flags() targets for Rust executable targets.",
            providers = [BuildFlagsInfo],
        ),
    },
)

#############################################################################
#############################################################################
#####
#####    rust_toolchain_with_build_flags()
#####
#####    Wraps a rust_toolchain() target to append default build flags from
#####    @fuchsia_rules_common//build_flags:toolchain_type.
#####

def _rust_toolchain_with_build_flags_impl(ctx):
    base_info = ctx.attr.toolchain[platform_common.ToolchainInfo]
    flags_toolchain = ctx.toolchains["@fuchsia_rules_common//build_flags:toolchain_type"]

    extra_flags = []
    if flags_toolchain:
        # Only use the common_infos list for Rust targets, since the GN default configs
        # for Rust do not change for executables or shared libraries.
        for info in flags_toolchain.default_flags.rust_common_infos:
            extra_flags.extend(info.rustflags)
            for lib_dir in info.lib_dirs:
                extra_flags.append("-Lnative=" + lib_dir)

    raw_flags = extra_flags + ctx.attr.extra_rustc_flags + getattr(base_info, "extra_rustc_flags", [])
    seen = {}
    all_extra_flags = []
    for flag in raw_flags:
        if flag not in seen:
            seen[flag] = True
            all_extra_flags.append(flag)

    fields = {}
    for k in dir(base_info):
        if k in ("to_json", "to_proto"):
            continue
        fields[k] = getattr(base_info, k)
    fields["extra_rustc_flags"] = all_extra_flags

    providers = [platform_common.ToolchainInfo(**fields)]
    if platform_common.TemplateVariableInfo in ctx.attr.toolchain:
        providers.append(ctx.attr.toolchain[platform_common.TemplateVariableInfo])

    return providers

rust_toolchain_with_build_flags = rule(
    implementation = _rust_toolchain_with_build_flags_impl,
    doc = "Wraps a rust_toolchain() target to append default build flags from @fuchsia_rules_common//build_flags:toolchain_type.",
    attrs = {
        "toolchain": attr.label(
            doc = "The base rust_toolchain() target.",
            mandatory = True,
            providers = [platform_common.ToolchainInfo],
        ),
        "extra_rustc_flags": attr.string_list(
            doc = "Extra flags to append after default build flags.",
            default = [],
        ),
    },
    toolchains = [
        config_common.toolchain_type(
            "@fuchsia_rules_common//build_flags:toolchain_type",
            mandatory = False,
        ),
    ],
)
