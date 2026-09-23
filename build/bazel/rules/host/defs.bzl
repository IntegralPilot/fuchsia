# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for host tools."""

load("@io_bazel_rules_go//go:def.bzl", "go_binary")
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("@rules_cc//cc:defs.bzl", "cc_binary")
load("@rules_python//python:defs.bzl", "py_binary")
load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
load("//build/bazel/rules/rust:defs.bzl", "rustc_binary")

# Use with the `target_compatible_with` attribute to declare that a target
# should only be built at the "PLATFORM" API level.
#
# Note that attempting to build the target in another configuration will result
# in a message that the target platform "didn't satisfy constraint
# @@platforms//:incompatible" and not mention the API level.
_target_compatible_with_platform_api_level = select({
    # If the API level is not "PLATFORM", the target is incompatible.
    "//build/bazel/versioning:is_api_level_PLATFORM": [],
    "//conditions:default": ["@platforms//:incompatible"],
})

def _cc_binary_host_tool_impl(
        name,
        target_compatible_with,
        **kwargs):
    if target_compatible_with != HOST_CONSTRAINTS and target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.")

    # Also ensure the API level is "PLATFORM".
    target_compatible_with += _target_compatible_with_platform_api_level

    cc_binary(
        name = name,
        target_compatible_with = target_compatible_with,
        **kwargs
    )

cc_binary_host_tool = macro(
    doc = """A cc_binary to be used as a host tool.

    Use for all `cc_binary()` targets that are host tools.

    While currently just a wrapper, it ensures appropriate constraints and
    enables flexibility in how we build host tools in the future, such as
    addressing https://fxbug.dev/486198435.
    """,
    implementation = _cc_binary_host_tool_impl,
    # rules_cc wraps the `native.cc_binary()` rule in a legacy macro, which
    # cannot be used with `inherit_attrs`. Use the rule directly instead.
    inherit_attrs = native.cc_binary,
    attrs = {
        "target_compatible_with": attr.string_list(
            doc = "Standard meaning. Must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.",
            mandatory = False,
            configurable = False,
            default = HOST_CONSTRAINTS,
        ),
    },
)

# This must be a legacy macro with `**kwargs` because rules_go wraps the
# `go_binary()` rule in a legacy macro, which cannot be used with
# `inherit_attrs` in a symbolic macro.
def go_binary_host_tool(
        *,
        name,
        target_compatible_with,
        **kwargs):
    """A go_binary to be used as a host tool.

    Use for all `go_binary()` targets that are host tools.

    While currently just a wrapper, it ensures appropriate constraints and
    enables flexibility in how we build host tools in the future, such as
    addressing https://fxbug.dev/486198435.

    Args:
        name: The name of the tool.
        target_compatible_with: Standard meaning. Must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.
        **kwargs: Passed to `go_binary()`.
    """
    if target_compatible_with != HOST_CONSTRAINTS and target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.")

    # Also ensure the API level is "PLATFORM".
    target_compatible_with += _target_compatible_with_platform_api_level

    go_binary(
        name = name,
        target_compatible_with = target_compatible_with,
        **kwargs
    )

# This must be a legacy macro with `**kwargs` because `py_binary()` in
# rules_python is a legacy macro, which cannot be used with `inherit_attrs` in
# a symbolic macro.
def py_binary_host_tool(
        *,
        name,
        target_compatible_with,
        **kwargs):
    """A py_binary to be used as a host tool.

    Use for all `py_binary()` targets that are host tools.

    While currently just a wrapper, it ensures appropriate constraints and
    enables flexibility in how we build host tools in the future, such as
    addressing https://fxbug.dev/486198435.

    Args:
        name: The name of the tool.
        target_compatible_with: Standard meaning. Must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.
        **kwargs: Passed to `py_binary()`.
    """
    if target_compatible_with != HOST_CONSTRAINTS and target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.")

    # Also ensure the API level is "PLATFORM".
    target_compatible_with += _target_compatible_with_platform_api_level

    py_binary(
        name = name,
        target_compatible_with = target_compatible_with,
        **kwargs
    )

def _rustc_binary_host_tool_impl(
        name,
        target_compatible_with,
        **kwargs):
    if target_compatible_with != HOST_CONSTRAINTS and target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.")

    # Also ensure the API level is "PLATFORM".
    target_compatible_with += _target_compatible_with_platform_api_level

    rustc_binary(
        name = name,
        target_compatible_with = target_compatible_with,
        **kwargs
    )

rustc_binary_host_tool = macro(
    doc = """A rust_binary to be used as a host tool.

    Use for all `rustc_binary()` targets that are host tools.

    While currently just a wrapper, it ensures appropriate constraints and
    enables flexibility in how we build host tools in the future, such as
    addressing https://fxbug.dev/486198435.
    """,
    implementation = _rustc_binary_host_tool_impl,
    inherit_attrs = rustc_binary,
    attrs = {
        "target_compatible_with": attr.string_list(
            doc = "Standard meaning. Must be `HOST_CONSTRAINTS` or `HOST_OS_CONSTRAINTS`.",
            mandatory = False,
            configurable = False,
            default = HOST_CONSTRAINTS,
        ),
    },
)
