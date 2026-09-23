# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tools to check the compatibility of fuchsia targets"""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")

_SUPPORTED_HOST_CPU_PLATFORMS = [
    "@platforms//cpu:x86_64",
]
_SUPPORTED_HOST_OS_PLATFORMS = [
    "@platforms//os:linux",
]

def _define_host_can_build_fuchsia_flag_impl(ctx):
    valid_host_cpu = any([cpu in HOST_CONSTRAINTS for cpu in _SUPPORTED_HOST_CPU_PLATFORMS])
    valid_host_os = any([os in HOST_CONSTRAINTS for os in _SUPPORTED_HOST_OS_PLATFORMS])
    fuchsia_targets_enabled = ctx.attr._fuchsia_targets_enabled[BuildSettingInfo].value

    if valid_host_cpu and valid_host_os and fuchsia_targets_enabled:
        valid_host = "yes"
    else:
        valid_host = "no"

    return [
        config_common.FeatureFlagInfo(value = valid_host),
    ]

define_host_can_build_fuchsia_flag = rule(
    doc = """A build value that can be used as a constraint.

    This build config allows us to select on whether the build is executing on a
    supported host platform. If we do not do this then glob builds will fail
    due on host machines we do not support due to not having a valid toolchain.
    """,
    implementation = _define_host_can_build_fuchsia_flag_impl,
    attrs = {
        "_fuchsia_targets_enabled": attr.label(
            # Note: we need to mark this as being on rules_fuchsia explicitly while
            # we continue to transition from fuchsia_sdk to rules_fuchsia
            default = "@rules_fuchsia//fuchsia/flags:fuchsia_targets_enabled",
        ),
    },
)

_HOST_CONDITION = select({
    "@rules_fuchsia//fuchsia/constraints:can_host_build_fuchsia": [],
    "//conditions:default": ["@platforms//:incompatible"],
})
_HOST_DEPS = ["@rules_fuchsia//fuchsia/constraints:check_host_compatibility"]
_FUCHSIA_DEPS = ["@rules_fuchsia//fuchsia/constraints:check_fuchsia_compatibility"]

# Different expressions of 2 categories of constraints:
#  - `COMPATIBILITY.HOST_*` constraints are all functionally equivalent reexpressions of one
#    another, restricting the exec build platform to linux-amd64 via `target_compatible_with`.
#  - `COMPATIBILITY.FUCHSIA_*` constraints are also functionally equivalent to one another, being a
#    superset of their host counterparts that also constrains target os to fuchsia.
#
# These constraints are useful to prevent glob expressions like `bazel build //...` from capturing
# targets in macos/arm64 hosts since the Bazel SDK doesn't support those host platforms.
# `COMPATIBILITY.FUCHSIA_*` constraints can additionally prevent non-fuchsia-invalid build
# configurations from being picked up from these glob expressions as well.
#
# Practically speaking, here are a few cases when `COMPATIBILITY.HOST_*` should be used:
#   1. A looser restriction than "COMPATIBILITY.FUCHSIA_*" is required.
#   2. The target needs to run any tool from the SDK, as those are only compatible for certain
#      host configurations.
#   3. Any implicit toolchains that the target needs that are only compatible with linux-amd64.
#   4. Any target that should otherwise be skipped for non-linux-amd64 hosts.
#      (eg: depends on a prebuilt that is only available for linux-amd64).
#
# Here are a few cases when `COMPATIBILITY.FUCHSIA_*` should be used instead:
#   1. Any implicit toolchains that the target needs that are only compatible with the fuchsia
#      target os. (eg: macros using `cc_*` that depend on `@fuchsia_clang//:cc-$ARCH`).
#   2. Dependent targets can only meaningfully use this target's output (providers/files) by
#      targeting Fuchsia. (See #1).
#   3. Any target that should otherwise be skipped when not targeting fuchsia.
#      (eg: some dependencies below the package level when it doesn't make sense to build them
#      outside of the context of a package).
COMPATIBILITY = struct(
    # A select() statement that returns a `target_compatible_with` value that will be `[]` if the
    # current host system is supported by the Fuchsia SDK cross-toolchain, or
    # `[ "@platforms//:incompatible" ]` otherwise.
    # This approach works well for template-generated targets and user-defined targets but is
    # not recommended for most macros since it's not possible to handle for user-specified
    # `target_compatible_with` value deduplication correctly in macros.
    #
    # Example usage:
    #
    #  ```
    # load("@rules_fuchsia//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
    #
    #  cc_library(
    #     name = "foo",
    #     ...
    #     target_compatible_with = COMPATIBILITY.HOST_CONDITION,
    #  )
    #  ```
    HOST_CONDITION = _HOST_CONDITION,

    # Use these deps to ensure that building targets and dependents propagate
    # `target_compatible_with` up the dependency chain, preventing incompatible exec build
    # configurations (non-linux-amd64) don't attempt to build these targets.
    #
    # Example usage:
    #
    # ```
    # load("@rules_fuchsia//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
    #
    # def foo_macro(deps = [], **kwargs):
    #     cc_library(
    #         deps = deps + COMPATIBILITY.HOST_DEPS,
    #         **kwargs
    #     )
    # ```
    HOST_DEPS = _HOST_DEPS,

    # Use this attribute dict partials in a rule definition to ensure all instantiated targets of
    # that rule and dependent targets of those instantiated targets have the constraints of
    # `COMPATIBILITY.HOST_DEPS` applied and propagated upwards.
    #
    # Example usage:
    # ```
    # load("@rules_fuchsia//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
    #
    # foo = rule(
    #     ...
    #     attrs = {
    #         ...
    #     } | COMPATIBILITY.HOST_ATTRS
    # )
    # ```
    HOST_ATTRS = {
        "_host_compatibility": attr.label_list(
            doc = """
    Ensures that the current (and all dependent targets') target build configuration corresponds to a compatible host.
    See `@rules_fuchsia//fuchsia/constraints:target_compatibility.bzl` > `COMPATIBILITY` for more context.
    """,
            default = _HOST_DEPS,
        ),
    },

    # The following `FUCHSIA_*` constraints are supersets of their corresponding `HOST_*`
    # constraints, adding an additional check that the target os is fuchsia.
    # For any `FUCHSIA_*` field, see its corresponding `HOST_*` field for more context.
    FUCHSIA_CONDITION = _HOST_CONDITION + ["@platforms//os:fuchsia"],
    FUCHSIA_DEPS = _FUCHSIA_DEPS,
    FUCHSIA_ATTRS = {
        "_fuchsia_compatibility": attr.label_list(
            doc = """
    Ensures that the current (and all dependent targets') target build configuration corresponds to a compatible host and target.
    See `@rules_fuchsia//fuchsia/constraints:target_compatibility.bzl` > `COMPATIBILITY` for more context.
    """,
            default = _FUCHSIA_DEPS,
        ),
    },
)
