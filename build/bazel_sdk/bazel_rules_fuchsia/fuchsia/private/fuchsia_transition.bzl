# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities for changing the build configuration to fuchsia."""

load("//common:transition_utils.bzl", "set_command_line_option_value")
load(
    "//fuchsia/constraints/platforms:supported_platforms.bzl",
    "ALL_SUPPORTED_PLATFORMS",
    "fuchsia_platforms",
)
load(
    ":fuchsia_api_level.bzl",
    "FUCHSIA_API_LEVEL_TARGET",
    "REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET",
    "u32_for_fuchsia_api_level_or_none",
)

NATIVE_CPU_ALIASES = {
    "darwin": "x86_64",
    "k8": "x86_64",
    "x86_64": "x86_64",
    "aarch64": "aarch64",
    "darwin_arm64": "aarch64",
    "darwin_x86_64": "x86_64",
    "riscv64": "riscv64",
}

FUCHSIA_PLATFORMS_MAP = {
    "x86_64": "fuchsia_x64",
    "aarch64": "fuchsia_arm64",
    "riscv64": "fuchsia_riscv64",
}

CPU_MAP = {
    fuchsia_platforms.x64: "x86_64",
    fuchsia_platforms.arm64: "aarch64",
    fuchsia_platforms.riscv64: "riscv64",
}

_REPO_DEFAULT_API_LEVEL_TARGET_NAME = "@fuchsia_sdk//flags:repository_default_fuchsia_api_level"

def _fuchsia_api_level_in_effect(settings, attr):
    # The logic for determining what API level to use.
    # The effective precedence is specified below.
    # None of the values have been validated at this point.
    # If the transition is executed multiple times, the previous result will be in (1).

    # 1. Check the value that is manually specified via command-line
    manually_specified_api_level = settings[FUCHSIA_API_LEVEL_TARGET]

    # 2. Check the value that is set on the fuchsia_package
    target_specified_api_level = getattr(attr, "fuchsia_api_level", None)

    # 3. Check the repository_default_fuchsia_api_level flag
    repo_default_api_level = settings[REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET]

    return (
        manually_specified_api_level
    ) or (
        target_specified_api_level
    ) or (
        repo_default_api_level
    ) or (
        # No API level has been specified. This will be handled by the
        # `fuchsia_api_level` implementation, which is cleaner than failing in
        # the transition and allows failure tests to be written.
        ""
    )

def _package_supplied_platform(attr):
    # We should be pulling the platform off of the package but we need to clean
    # up the usages of this transition before we can assume the target of the
    # transition is always a package.
    if hasattr(attr, "platform"):
        platform = attr.platform

        # if platform is not set fwe fall back to our old method for finding the
        # platform until we transition all users.
        if platform != None and platform != "":
            if platform in ALL_SUPPORTED_PLATFORMS:
                return platform
            else:
                fail("ERROR: Attempting to build a fuchsia package with an unsupported platform: ", platform)

    return None

def _fuchsia_transition_impl(settings, attr):
    fuchsia_platform = _package_supplied_platform(attr)

    if fuchsia_platform == None:
        input_cpu = settings["//command_line_option:cpu"]
        output_cpu = NATIVE_CPU_ALIASES.get(input_cpu, None)
    else:
        output_cpu = CPU_MAP[fuchsia_platform]

    if not output_cpu:
        fail("Unrecognized cpu %s." % input_cpu)

    # allow for a soft transition
    if fuchsia_platform == None:
        fuchsia_platform = "//fuchsia/constraints/platforms:" + FUCHSIA_PLATFORMS_MAP[output_cpu]

    copt = settings["//command_line_option:copt"] + (
        [] if "--debug" in settings["//command_line_option:copt"] else ["--debug"]
    )

    # Add the Fuchsia API level to the Clang command line.
    # If the API level is not supported, the command line will be broken, but the invalid level
    # should be caught before it is used.
    fuchsia_api_level = _fuchsia_api_level_in_effect(settings, attr)
    copt = set_command_line_option_value(copt, "-ffuchsia-api-level=", str(u32_for_fuchsia_api_level_or_none(fuchsia_api_level)))

    return {
        "//command_line_option:cpu": output_cpu,
        "//command_line_option:copt": copt,
        "//command_line_option:strip": "never",
        "//command_line_option:platforms": fuchsia_platform,
        FUCHSIA_API_LEVEL_TARGET: fuchsia_api_level,
    }

fuchsia_transition = transition(
    implementation = _fuchsia_transition_impl,
    inputs = [
        FUCHSIA_API_LEVEL_TARGET,
        REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET,
        "//command_line_option:cpu",
        "//command_line_option:copt",
    ],
    outputs = [
        FUCHSIA_API_LEVEL_TARGET,
        "//command_line_option:cpu",
        "//command_line_option:copt",
        "//command_line_option:strip",
        "//command_line_option:platforms",
    ],
)
