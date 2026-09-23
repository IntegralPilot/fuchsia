# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")
load(
    ":build_flags.bzl",
    "compute_final_build_flags_from",
)
load(":providers.bzl", "BuildFlagsInfo", "BuildFlagsListInfo")

# Common attributes for all Rust rules that support build_flags().
# Two important points:
#
# - disable_build_flags is not supported for Rust targets due to
#   @rules_rust limitations. See README.md for details.
#
# - build_flags is non-configurable to work-around the fact that
#   include_str! directives with relative paths that start with
#   ../ do not work properly. See https://fxbug.dev/516778625
BUILD_FLAGS_RUST_ATTRS_KWARGS = {
    "build_flags": attr.label_list(
        doc = "List of build_flags() targets.",
        providers = [BuildFlagsInfo],
        configurable = False,
        default = [],
    ),
}

# The set of valid Rust "target_type" values for wrap_rust_macro_args_with_build_flags()
BUILD_FLAGS_RUST_TARGET_TYPES = set(["rust_common", "rust_executable", "rust_shared_library"])

# LINT.IfChange(rust_action_kinds)
ACTION_KIND_RUST_COMPILE = "rust_compile"

RUST_ACTION_KINDS = [
    ACTION_KIND_RUST_COMPILE,
]
# LINT.ThenChange(//build/bazel/scripts/bazel_build_args.py:rust_action_kinds)

#############################################################################
#############################################################################
#####
#####    wrap_rust_rule_with_build_flags()
#####

_final_rust_build_flags = rule(
    doc = "Provides the final ordered list of BuildFlagsInfo values. This is used " +
          "to generate response files for different action types.",
    provides = [BuildFlagsListInfo],
    attrs = {
        "build_flags": attr.label_list(
            doc = "List of build_flags() target labels.",
            providers = [BuildFlagsInfo],
            default = [],
        ),
        "target_type": attr.string(
            doc = "The type of target being wrapped.",
            mandatory = True,
            values = list(BUILD_FLAGS_RUST_TARGET_TYPES),
        ),
    },
    implementation = lambda ctx: [
        # For now this basic implementation is enough.
        BuildFlagsListInfo(
            infos = compute_final_build_flags_from(
                [target[BuildFlagsInfo] for target in ctx.attr.build_flags],
                [],  # no disable_build_flags support in Rust for now.
            ),
        ),
    ],
)

def _compute_build_flags_for_rust_action(build_flags_infos, action_kind):
    """Compute the list of build flags for a given action kind.

    Args:
        build_flags_infos: A list of BuildFlagsInfo providers.
        action_kind: The kind of action to compute build flags for.
          Must be one of the ACTION_KIND_XXX constants defined in this module.

    Returns:
        A string list containing the build flags for the given action kind.
    """
    result = []
    if action_kind == ACTION_KIND_RUST_COMPILE:
        for info in build_flags_infos:
            result.extend(info.rustflags)
        for info in build_flags_infos:
            result.extend(["-Lnative={}".format(lib_dir) for lib_dir in info.lib_dirs])
    else:
        fail("Unsupported Rust action kind {}, must be one of: {}".format(
            action_kind,
            ", ".join(RUST_ACTION_KINDS),
        ))
    return result

def _rust_response_file_internal_impl(ctx):
    build_flags_infos = ctx.attr.final_build_flags[BuildFlagsListInfo].infos
    flags = _compute_build_flags_for_rust_action(build_flags_infos, ctx.attr.action_kind)
    output = ctx.actions.declare_file(ctx.label.name)
    args = ctx.actions.args()
    args.set_param_file_format("multiline")
    args.add_all(flags)
    ctx.actions.write(output, args)

    # Ensure the response files are part of the compiler sandbox by
    # making them available through the CcInfo provider.
    # TODO(https://fxbug.dev/1650836) A cleaner way would be to use additional_compiler_inputs,
    # but this attribute is only available from Bazel 9.1.
    compilation_context = cc_common.create_compilation_context(
        headers = depset([output]),
    )
    return [
        DefaultInfo(files = depset([output])),
        CcInfo(compilation_context = compilation_context),
    ]

_rust_response_file_internal = rule(
    doc = "Generate a response file containing build flags for a given Rust action.",
    implementation = _rust_response_file_internal_impl,
    attrs = {
        "action_kind": attr.string(
            doc = "The kind of Rust action to generate build flags for.",
            mandatory = True,
            values = RUST_ACTION_KINDS,
        ),
        "final_build_flags": attr.label(
            doc = "A _final_rust_build_flags() target label.",
            mandatory = True,
            providers = [BuildFlagsListInfo],
        ),
    },
)

def _rust_response_file(target_name, action_kind, final_build_flags, testonly):
    """Generate a new response file containing build_flags() flags for a given action kind.

    Args:
        target_name: Name of the wrapped target that will use the response file.
        action_kind: The kind of Rust action to generate a response file for.
        final_build_flags: The label of a _final_rust_build_flags() target to use.
        testonly: Whether the response file target should be testonly.

    Returns:
        The label of the new response file target.
    """
    name = "{}.{}.build_flags".format(target_name, action_kind)
    _rust_response_file_internal(
        name = name,
        action_kind = action_kind,
        final_build_flags = final_build_flags,
        testonly = testonly,
    )
    return name

def _rustc_env_file_internal_impl(ctx):
    build_flags_infos = ctx.attr.final_build_flags[BuildFlagsListInfo].infos
    env_vars = []
    for info in build_flags_infos:
        env_vars.extend(info.rustenv)

    output = ctx.actions.declare_file(ctx.label.name)
    args = ctx.actions.args()
    args.set_param_file_format("multiline")
    args.add_all(env_vars)
    ctx.actions.write(output, args)
    return [DefaultInfo(files = depset([output]))]

_rustc_env_file_internal = rule(
    doc = "Generate a file containing NAME=value definitions for rustc environment variables.",
    implementation = _rustc_env_file_internal_impl,
    attrs = {
        "final_build_flags": attr.label(
            doc = "A _final_rust_build_flags() target label.",
            mandatory = True,
            providers = [BuildFlagsListInfo],
        ),
    },
)

def _rustc_env_file(target_name, final_build_flags, testonly):
    name = "{}.rustc_env_file.build_flags".format(target_name)
    _rustc_env_file_internal(
        name = name,
        final_build_flags = final_build_flags,
        testonly = testonly,
    )
    return name

def wrap_rust_macro_args_with_build_flags(
        kwargs,
        name,
        rust_rule_name,  # buildifier: disable=unused-variable
        build_flags,
        target_type):
    """Wrap the keyword arguments of a given rust_xxx() wrapping macro.

    This is useful in macros that need to wrap regular rust_xxxx() rules
    to add support for with_build_flags and without_build_flags.

    Example usage:
        ```
        def my_rust_library(name, build_flags = [], **kwargs):
            new_kwargs = wrap_rust_macro_args_with_build_flags(
                kwargs = kwargs,
                name = name,
                rust_rule_name = "rust_library",
                build_flags = build_flags,
                target_type = "common",
            )

            rust_library(
                name = name,
                **new_kwargs,
            )
        ```

    Args:
       kwargs: (dict) Keyword-argument dictionary of wrapper macro.
       name: (string) Target name
       rust_rule_name: (string) Name of wrapped rust_xxxx() rule.
       build_flags: (list[string]) List of build_flags() labels.
       target_type: (string) The type of target being wrapped, must
           be one of "rust_common", "rust_executable" or "rust_shared_library".
    Returns:
       A new keyword-argument with updated values.
    """
    if target_type not in BUILD_FLAGS_RUST_TARGET_TYPES:
        fail("Invalid target_type value ({}), should be one of: {}".format(
            target_type,
            ", ".join(BUILD_FLAGS_RUST_TARGET_TYPES),
        ))

    if not build_flags:
        # Note that this check will not work when the lists are empty
        # `select("//conditions/default": [])` values, which happens
        # when the attributes are defined without `configurable = False`
        # in the respective rustc_xxxx() macro.
        #
        # There is no way to inspect the content of a select() value
        # in a macro. This code path is only here due to
        # https://fxbug.dev/516778625 and will be removed once it is fixed.
        return kwargs

    testonly = kwargs.get("testonly", False)

    # Compute the final set of build flags for this target.
    final_build_flags_name = name + ".final_build_flags"
    _final_rust_build_flags(
        name = final_build_flags_name,
        build_flags = build_flags,
        target_type = target_type,
        testonly = testonly,
    )

    # Generate response files for all action types that may be generated by rust targets.
    # This response file will include rustflags, and its
    # path will be added to the result's compile_data list to make it available
    # to the rust compiler's sandbox.
    rustc_flags_response_name = _rust_response_file(
        target_name = name,
        action_kind = ACTION_KIND_RUST_COMPILE,
        final_build_flags = final_build_flags_name,
        testonly = testonly,
    )

    # This env file contains the rustenv environment variable definitions
    # that need to be passed to the rustc action.
    rustc_env_file_name = _rustc_env_file(
        target_name = name,
        final_build_flags = final_build_flags_name,
        testonly = testonly,
    )

    # Add the response files to the corresponding action attributes.
    result = dict(kwargs)
    result["rustc_flags"] = ["@$(location {})".format(rustc_flags_response_name)] + (kwargs.get("rustc_flags") or [])
    result["rustc_env_files"] = (kwargs.get("rustc_env_files") or []) + [":" + rustc_env_file_name]

    result["compile_data"] = (kwargs.get("compile_data") or []) + [
        ":" + rustc_flags_response_name,
    ]

    return result
