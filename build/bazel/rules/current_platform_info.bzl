# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Allow rule implementation functions to access the current build configuration's
# operating system and cpu architecture names, using Fuchsia conventions.
#
# Sadly, it seems that it is not possible to directly access these using standard
# Bazel mechanism, because constraint settings and constraint values do not act
# as build settings.

CurrentPlatformInfo = provider(
    doc = """A provider exposing the current build configuration's os and cpu names.""",
    fields = {
        "os": "Target operating system, using Fuchsia conventions.",
        "cpu": "Target cpu architecture, using Fuchsia conventions.",
    },
)

def _current_platform_info_impl(ctx):
    return [
        CurrentPlatformInfo(
            os = ctx.attr.os,
            cpu = ctx.attr.cpu,
        ),
    ]

# Arguments to the select() statements used within define_current_platform_info().
# Add new entries here to support new operating systems and cpu architectures.
_CURRENT_PLATFORM_SELECT_OS = {
    "@platforms//os:fuchsia": "fuchsia",
    "@platforms//os:linux": "linux",
    "@platforms//os:android": "android",
    # Let Bazel error in case of unknown OS.
}

_CURRENT_PLATFORM_SELECT_CPU = {
    "@platforms//cpu:x86_64": "x64",
    "@platforms//cpu:aarch64": "arm64",
    "@platforms//cpu:riscv64": "riscv64",
    # Let Bazel error in case of unknown CPU.
}

_current_platform_info = rule(
    doc = """Provide OS and CPU info for the current build configuration.""",
    implementation = _current_platform_info_impl,
    provides = [CurrentPlatformInfo],
    attrs = {
        "os": attr.string(
            doc = "platform os name, following Fuchsia convention.",
            mandatory = True,
        ),
        "cpu": attr.string(
            doc = "target cpu name, following Fuchsia convention.",
            mandatory = True,
        ),
    },
)

def define_current_platform_info(*, name):
    """Define a target providing a CurrentPlatformInfo provider.

    Dependents of this target will be able to access the provider
    to get Fuchsia-named OS and CPU architecture names for the
    current build configuration.

    Example usage:

        load("@//build/bazel/rules:current_platform_info.bzl", "CurrentPlatformInfo")

        def _my_rule_impl(ctx):
            current_platform = ctx._current_platform[CurrentPlatformInfo]
            print("TARGET=%s OS=%s CPU=%s" % (ctx.label, current_platform.os, current_platform.cpu))
           ...

        my_rule = rule(
            attrs = {
               ...
               "_current_platform": attr.label(
                    default = "@//build/bazel:current_platform",
                    providers = [CurrentPlatformInfo],
               ),
            },
        )

    Args:
        name: target name.
    """
    _current_platform_info(
        name = name,
        os = select(_CURRENT_PLATFORM_SELECT_OS),
        cpu = select(_CURRENT_PLATFORM_SELECT_CPU),
        visibility = ["//visibility:public"],
    )
