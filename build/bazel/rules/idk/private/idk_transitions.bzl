# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Transitions used to build the IDK."""

load("@fuchsia_build_info//:args.bzl", "idk_buildable_api_levels", "idk_buildable_cpus", "target_cpu")

visibility([
    "//build/bazel/rules/idk/...",
    "//sdk",
])

def _verify_is_main_platform_configuration(current_platforms, current_api_level):
    main_target_platform = "//build/bazel/platforms:fuchsia_platform_%s" % target_cpu

    if (len(current_platforms) != 1 or
        str(current_platforms[0]) != ("@@" + main_target_platform) or
        current_api_level != "PLATFORM"):
        fail('This transition should only be used in the main "PLATFORM" build (platform: "%s", api_level: "PLATFORM"), not (platforms: "%s", api_level: "%s").' % (
            main_target_platform,
            current_platforms,
            current_api_level,
        ))

def _verify_is_main_platform_configuration_from_settings(settings):
    _verify_is_main_platform_configuration(
        settings["//command_line_option:platforms"],
        settings["@//build/bazel/versioning:api_level"],
    )

def _api_level_and_cpu_combinations_transition_impl(settings, _attr):
    _verify_is_main_platform_configuration_from_settings(settings)

    combinations = []

    # In addition to the API levels supported by the IDK, we also build for
    # "PLATFORM" for CPU architectures other than `target_cpu` to populate
    # `arch/` in the IDK.
    # TODO(https://fxbug.dev/310006516): Remove `+ ["PLATFORM"]` once `arch/`
    # is removed from the IDK.
    for api_level in idk_buildable_api_levels + ["PLATFORM"]:
        for cpu in idk_buildable_cpus:
            if api_level == "PLATFORM" and cpu == target_cpu:
                # This configuration is already covered by the main platform build.
                continue

            combinations.append({
                "@//build/bazel/versioning:api_level": api_level,
                "//command_line_option:platforms": "@//build/bazel/platforms:fuchsia_platform_%s" % cpu,
            })
    return combinations

# A 1:n transition to configurations for each combination of CPU architecture and API level.
api_level_and_cpu_combinations_transition = transition(
    implementation = _api_level_and_cpu_combinations_transition_impl,
    inputs = [
        "@//build/bazel/versioning:api_level",
        "//command_line_option:platforms",
    ],
    outputs = [
        "@//build/bazel/versioning:api_level",
        "//command_line_option:platforms",
    ],
)

def _build_in_all_idk_api_level_and_cpu_combinations_impl(ctx):
    if not ctx.attr.testonly:
        fail("This rule is only intended to be used in tests.")

    all_deps_depset = depset(direct = ctx.files.deps)

    return [
        DefaultInfo(files = all_deps_depset),
    ]

# Builds the specified targets for each combination of IDK buildable API level
# and CPU architecture.
build_in_all_idk_api_level_and_cpu_combinations = rule(
    implementation = _build_in_all_idk_api_level_and_cpu_combinations_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Targets to build for each combination of API level and CPU architecture.",
            cfg = api_level_and_cpu_combinations_transition,
        ),
    },
)

# TODO(https://fxbug.dev/442025401): Add build settings we do not want to carry
# over from the Fuchsia configuration.
BUILD_SETTINGS_TO_RESET_FOR_HOST_TOOLS = [
]

BUILD_SETTINGS_RESET_FOR_HOST_TOOLS_VALUES = {
}

def _get_host_platform_cpu(host_cpu):
    """Returns the CPU string, following Fuchsia conventions, for the host CPU architecture.

    Args:
        host_cpu: The host CPU architecture following Bazel conventions, as a string.

    Returns:
        The host CPU architecture following Fuchsia conventions, as a string.
    """
    if host_cpu == "k8":
        return "x64"
    elif host_cpu == "aarch64":
        return "arm64"
    else:
        fail("Unsupported host CPU architecture: '%s'" % host_cpu)

def _current_host_cpu_transition_impl(settings, _attr):
    _verify_is_main_platform_configuration(
        settings["//command_line_option:platforms"],
        settings["@//build/bazel/versioning:api_level"],
    )

    host_platform_cpu = _get_host_platform_cpu(settings["//command_line_option:host_cpu"])
    host_platform_os = "linux"

    return {
        "//command_line_option:platforms": "@//build/bazel/platforms:host_%s_%s" % (host_platform_os, host_platform_cpu),
    } | BUILD_SETTINGS_RESET_FOR_HOST_TOOLS_VALUES

current_host_cpu_transition = transition(
    implementation = _current_host_cpu_transition_impl,
    inputs = [
        "@//build/bazel/versioning:api_level",
        "//command_line_option:host_cpu",
        "//command_line_option:platforms",
    ],
    outputs = ["//command_line_option:platforms"] + BUILD_SETTINGS_TO_RESET_FOR_HOST_TOOLS,
)

def _configured_host_cpus_transition_impl(settings, _attr):
    _verify_is_main_platform_configuration(
        settings["//command_line_option:platforms"],
        settings["@//build/bazel/versioning:api_level"],
    )

    host_platform_os = "linux"

    # TODO(https://fxbug.dev/442025401): Make this conditional on `sdk_cross_compile_host_tools`.
    # Consider merging this function with `_current_host_cpu_transition_impl()`.
    return [
        {
            "//command_line_option:platforms": "@//build/bazel/platforms:host_%s_arm64" % host_platform_os,
        } | BUILD_SETTINGS_RESET_FOR_HOST_TOOLS_VALUES,
        {
            "//command_line_option:platforms": "@//build/bazel/platforms:host_%s_x64" % host_platform_os,
        } | BUILD_SETTINGS_RESET_FOR_HOST_TOOLS_VALUES,
    ]

configured_host_cpus_transition = transition(
    implementation = _configured_host_cpus_transition_impl,
    inputs = [
        "@//build/bazel/versioning:api_level",
        "//command_line_option:host_cpu",
        "//command_line_option:platforms",
    ],
    outputs = ["//command_line_option:platforms"] + BUILD_SETTINGS_TO_RESET_FOR_HOST_TOOLS,
)
