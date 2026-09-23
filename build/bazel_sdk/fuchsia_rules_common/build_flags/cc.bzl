# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Implementation of the build_flags() rule and related definitions for cc targets.

See README.md file for full technical details.
"""

load("@rules_cc//cc:action_names.bzl", "ACTION_NAMES")
load(
    "@rules_cc//cc:cc_toolchain_config_lib.bzl",
    "feature",
    "flag_group",
    "flag_set",
)
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")
load(
    ":providers.bzl",
    "BuildFlagsInfo",
    "BuildFlagsListInfo",
)

# Common attributes for all C++ rules that support build_flags().
BUILD_FLAGS_CC_ATTRS_KWARGS = {
    "build_flags": attr.label_list(
        doc = "List of `build_flags()` targets.",
        providers = [BuildFlagsInfo],
        default = [],
    ),
    "disable_build_flags": attr.label_list(
        doc = "List of `build_flags()` targets whose flags should be excluded.",
        providers = [BuildFlagsInfo],
        default = [],
    ),
}

# The set of valid "target_type" values for wrap_cc_macro_args_with_build_flags()
BUILD_FLAGS_CC_TARGET_TYPES = set(["cxx_common", "cxx_executable", "cxx_shared_library"])

# Constants used to identify the type of actions that require build flags.
# These are NOT the @rules_cc//cc:action_names.bzl names.
# LINT.IfChange(cc_action_kinds)
ACTION_KIND_CPP_COMPILE = "cpp_compile"
ACTION_KIND_C_COMPILE = "c_compile"
ACTION_KIND_CPP_LINK = "cpp_link"

CC_ACTION_KINDS = [
    ACTION_KIND_CPP_COMPILE,
    ACTION_KIND_C_COMPILE,
    ACTION_KIND_CPP_LINK,
]
# LINT.ThenChange(//build/bazel/scripts/bazel_build_args.py:cc_action_kinds)

#############################################################################
#############################################################################
#####
#####    _final_cc_build_flags(), _compute_build_flags_for_cc_action() and
#####    _cc_response_file()
#####

def _final_cc_build_flags_impl(ctx):
    # Compute the default build_flags + the target-specific ones first.
    build_flags_infos = []
    toolchain = ctx.toolchains["@fuchsia_rules_common//build_flags:toolchain_type"]
    if toolchain:
        target_type = ctx.attr.target_type
        if target_type == "cxx_common":
            build_flags_infos.extend(toolchain.default_flags.cxx_common_infos)
        elif target_type == "cxx_executable":
            build_flags_infos.extend(toolchain.default_flags.cxx_common_infos)
            build_flags_infos.extend(toolchain.default_flags.cxx_executable_infos)
        elif target_type == "cxx_shared_library":
            build_flags_infos.extend(toolchain.default_flags.cxx_common_infos)
            build_flags_infos.extend(toolchain.default_flags.cxx_shared_library_infos)
        else:
            # Should never happen due to the 'values' list for the "target_type" attribute definition.
            fail("Invalid target_type {}".format(target_type))

    build_flags_infos += [target[BuildFlagsInfo] for target in ctx.attr.build_flags]

    # De-deduplicate and remove entries from disabled_build_flags
    disabled_labels = [target[BuildFlagsInfo].label for target in ctx.attr.disable_build_flags]
    known_labels = set(disabled_labels)

    final_infos = []
    for info in build_flags_infos:
        if info.label not in known_labels:
            known_labels.add(info.label)
            final_infos.append(info)

    return [BuildFlagsListInfo(infos = final_infos)]

_final_cc_build_flags = rule(
    implementation = _final_cc_build_flags_impl,
    doc = "Provides the final ordered list of BuildFlagsInfo values. This is used " +
          "to generate response files for different C++ action types.",
    provides = [BuildFlagsListInfo],
    attrs = {
        "target_type": attr.string(
            doc = "The type of target being wrapped.",
            mandatory = True,
            values = list(BUILD_FLAGS_CC_TARGET_TYPES),
        ),
    } | BUILD_FLAGS_CC_ATTRS_KWARGS,

    # Used to find the list of default build_flags() per target type.
    # See @fuchsia_rules_common//build_flags:{host,fuchsia}_default_build_flags_toolchain
    # for exact definitions. This must be optional because OOT SDK projects
    # will not register these toolchains in their top-level MODULE.bazel file.
    # This is ok, as build_flags() are only available within the Fuchsia source tree.
    toolchains = [
        config_common.toolchain_type(
            "@fuchsia_rules_common//build_flags:toolchain_type",
            mandatory = False,
        ),
    ],
)

def _compute_build_flags_for_cc_action(build_flags_infos, action_kind):
    """Compute the list of build flags for a given action kind.

    Args:
        build_flags_infos: A list of BuildFlagsInfo providers.
        action_kind: The kind of action to compute build flags for.
          Must be one of the ACTION_KIND_XXX constants defined in this module.

    Returns:
        A string list containing the build flags for the given action kind.
    """
    result = []
    if action_kind == ACTION_KIND_CPP_COMPILE:
        # NOTE: The GN toolchain definition in clang_toolchain() uses
        # {{defines}} {{include_dirs}} {{cflags}} {{cflags_cc}}
        for info in build_flags_infos:
            result.extend(["-D{}".format(define) for define in info.defines])
        for info in build_flags_infos:
            result.extend(["-I{}".format(include_dir) for include_dir in info.include_dirs])
        for info in build_flags_infos:
            result.extend(info.cflags)
        for info in build_flags_infos:
            result.extend(info.cflags_cc)
    elif action_kind == ACTION_KIND_C_COMPILE:
        # NOTE: The GN toolchain definition in clang_toolchain() uses
        # {{defines}} {{include_dirs}} {{cflags}} {{cflags_c}}
        for info in build_flags_infos:
            result.extend(["-D{}".format(define) for define in info.defines])
        for info in build_flags_infos:
            result.extend(["-I{}".format(include_dir) for include_dir in info.include_dirs])
        for info in build_flags_infos:
            result.extend(info.cflags)
        for info in build_flags_infos:
            result.extend(info.cflags_c)
    elif action_kind == ACTION_KIND_CPP_LINK:
        for info in build_flags_infos:
            result.extend(["-L{}".format(lib_dir) for lib_dir in info.lib_dirs])
            result.extend(info.ldflags)
    else:
        fail("Unsupported C++ action kind {}, must be one of: {}".format(
            action_kind,
            ", ".join(CC_ACTION_KINDS),
        ))
    return result

def _cc_response_file_internal_impl(ctx):
    build_flags_infos = ctx.attr.final_build_flags[BuildFlagsListInfo].infos
    flags = _compute_build_flags_for_cc_action(build_flags_infos, ctx.attr.action_kind)
    output = ctx.actions.declare_file(ctx.label.name)
    args = ctx.actions.args()
    args.set_param_file_format("shell")
    args.add_all(flags)
    ctx.actions.write(output, args)

    # Ensure the response files are part of the compiler sandbox by
    # making them available through the CcInfo provider. A cleaner way
    # would be to use additional_compiler_inputs, but this attribute is
    # only available from Bazel 9.1. See TODO(https://fxbug.dev/1650836)
    # and related comment below in wrap_cc_rule_with_build_flags().
    compilation_context = cc_common.create_compilation_context(
        headers = depset([output]),
    )
    return [
        DefaultInfo(files = depset([output])),
        CcInfo(compilation_context = compilation_context),
    ]

_cc_response_file_internal = rule(
    doc = "Generate a response file containing build flags for a given C++ action kind.",
    implementation = _cc_response_file_internal_impl,
    attrs = {
        "action_kind": attr.string(
            doc = "The kind of C++ action to generate build flags for.",
            mandatory = True,
            values = CC_ACTION_KINDS,
        ),
        "final_build_flags": attr.label(
            doc = "A _final_cc_build_flags() target label.",
            mandatory = True,
            providers = [BuildFlagsListInfo],
        ),
    },
)

def _cc_response_file(target_name, action_kind, final_build_flags, testonly):
    """Generate a new response file containing build_flags() flags for a given action kind.

    Args:
        target_name: Name of the wrapped target that will use the response file.
        action_kind: The kind of C++ action to generate a response file for.
        final_build_flags: The label of a _final_cc_build_flags() target to use.
        testonly: Whether the response file target should be testonly.

    Returns:
        The label of the new response file target.
    """
    name = "{}.{}.build_flags".format(target_name, action_kind)
    _cc_response_file_internal(
        name = name,
        action_kind = action_kind,
        final_build_flags = final_build_flags,
        testonly = testonly,
    )
    return name

#############################################################################
#############################################################################
#####
#####    wrap_cc_macro_args_with_build_flags()
#####

# The name of a feature() that the C++ toolchain will use to add all flags
# corresponding to the default build_flags() list. The feature will be disabled
# explicitly by wrap_cc_macro_args_with_build_flags() to make 'without_build_flags'
# work properly when it references a default build_flags() label.
BUILD_FLAGS_FEATURE_NAME = "fuchsia_default_build_flags"

def wrap_cc_macro_args_with_build_flags(
        *,
        kwargs,
        name,
        cc_rule_name,  # buildifier: disable=unused-variable
        build_flags,
        disable_build_flags,
        target_type):
    """Wrap the keyword arguments of a given `cc_xxx()` wrapping macro.

    This is useful in macros that need to wrap regular `cc_xxx()` rules
    to add support for `build_flags` and `disable_build_flags`.

    IMPORTANT: The input 'kwargs' must include all attributes supported by the
    wrapped `cc_xxx()` rule for the wrapping to work correctly.

    Example usage:
        ```
        def my_cc_library(
            name,
            build_flags = [],
            disable_build_flags = [],
            **kwargs,
        ):
            new_kwargs = wrap_cc_macro_args_with_build_flags(
                kwargs = kwargs,
                name = name,
                cc_rule_name = "cc_library",
                build_flags = build_flags,
                disable_build_flags = disable_build_flags,
                target_type = "common",
            )

            cc_library(
                name = name,
                **new_kwargs,
            )
        ```

    Args:
       kwargs: (dict) Keyword-argument dictionary of wrapper macro.
       name: (string) Target name
       cc_rule_name: (string) Name of wrapped cc_xxxx() rule (used for debugging)
       build_flags: (list[label]) List of build_flags() labels.
       disable_build_flags: (list[label]) List of build_flags() labels to disable.
       target_type: (string) The type of target being wrapped, must
          be one of "cxx_common", "cxx_executable" or "cxx_shared_library".
    Returns:
       (dict) A new keyword-argument with updated values.
    """

    if target_type not in BUILD_FLAGS_CC_TARGET_TYPES:
        fail("Invalid target_type value ({}), should be one of: {}".format(
            target_type,
            ", ".join(BUILD_FLAGS_CC_TARGET_TYPES),
        ))

    # NOTE: The following is commented out because build_flags and disable_build_flags are
    # configurable attributes, their default value is something like
    # select({"//conditions:default": []}) and there is no way to look
    # inside the select() statement to exit early if nothing is selected.
    #
    #if not build_flags and not disable_build_flags: return kwargs

    testonly = kwargs.get("testonly", False)

    # Compute the final set of build flags for this target.
    final_build_flags_name = name + ".final_build_flags"
    _final_cc_build_flags(
        name = final_build_flags_name,
        build_flags = build_flags,
        disable_build_flags = disable_build_flags,
        target_type = target_type,
        testonly = testonly,
    )

    cxx_response_name = _cc_response_file(
        target_name = name,
        action_kind = ACTION_KIND_CPP_COMPILE,
        final_build_flags = final_build_flags_name,
        testonly = testonly,
    )

    conly_response_name = _cc_response_file(
        target_name = name,
        action_kind = ACTION_KIND_C_COMPILE,
        final_build_flags = final_build_flags_name,
        testonly = testonly,
    )

    # Add the response files to the corresponding action attributes.
    result = dict(kwargs)

    result["cxxopts"] = ["@$(location {})".format(cxx_response_name)] + (kwargs.get("cxxopts") or [])
    result["conlyopts"] = ["@$(location {})".format(conly_response_name)] + (kwargs.get("conlyopts") or [])

    # TODO(https://fxbug.dev/1650836). Use "additional_compiler_inputs" when Bazel 9.1 or higher
    # is used (Fuchsia hasn't upgraded yet). In the meantime, the workaround is to export CcInfo
    # providers from the response file targets and add them to 'deps'.
    result["deps"] = (kwargs.get("deps") or []) + [
        ":" + cxx_response_name,
        ":" + conly_response_name,
    ]

    # Ensure the special BUILD_FLAGS_FEATURE_NAME feature is disabled, since this wrapping
    # will add all the default flags, while removing the disabled ones.
    result["features"] = (kwargs.get("features") or []) + ["-{}".format(BUILD_FLAGS_FEATURE_NAME)]

    # Only generate and apply linker flags for targets that actually link (executables and shared libraries)
    if target_type != "cxx_common":
        link_response_name = _cc_response_file(
            target_name = name,
            action_kind = ACTION_KIND_CPP_LINK,
            final_build_flags = final_build_flags_name,
            testonly = testonly,
        )
        result["linkopts"] = ["@$(location {})".format(link_response_name)] + (kwargs.get("linkopts") or [])
        result["additional_linker_inputs"] = (kwargs.get("additional_linker_inputs") or []) + [
            ":" + link_response_name,
        ]

    return result

#############################################################################
#############################################################################
#####
#####    compute_cc_toolchain_feature_for_default_build_flags()
#####
#####

def compute_cc_toolchain_feature_for_default_build_flags(
        default_flags_set):
    """Compute a C++ toolchain feature() for default build_flags() labels.

    Args:
      default_flags_set: (DefaultBuildFlagsSetInfo) a set of lists of build flags by
        target type.

    Returns:
      A new Bazel feature() object, enabled by default, which injects the appropriate
      compiler and linker flags for different action types.
    """

    def define_flag_set(actions, flags):
        """Return a flag_set() for a simple list of flags and set of actions.

        Args:
           actions: (list[str]) list of C++ action names.
           flags: (list[str]) list of command-line flags. These are interpreted
                verbatim without any type of Make variable expansion.
        Returns:
           A new flag_set() value.
        """

        # NOTE: Bazel complains when using 'flag_group(flags = [])' hence
        # the need for the "... if flags else []" expression below.
        return flag_set(
            actions = actions,
            flag_groups = [
                flag_group(flags = flags),
            ] if flags else [],
        )

    conly_compile_flags = _compute_build_flags_for_cc_action(
        default_flags_set.cxx_common_infos,
        ACTION_KIND_C_COMPILE,
    )
    conly_compile_actions = [
        ACTION_NAMES.c_compile,
        ACTION_NAMES.objc_compile,
    ]
    cxx_compile_flags = _compute_build_flags_for_cc_action(
        default_flags_set.cxx_common_infos,
        ACTION_KIND_CPP_COMPILE,
    )
    cxx_compile_actions = [
        ACTION_NAMES.linkstamp_compile,
        ACTION_NAMES.cpp_compile,
        ACTION_NAMES.cpp_header_parsing,
        ACTION_NAMES.cpp_module_compile,
        ACTION_NAMES.cpp_module_codegen,
        ACTION_NAMES.lto_backend,
        ACTION_NAMES.clif_match,
    ]

    common_link_flags = _compute_build_flags_for_cc_action(
        default_flags_set.cxx_common_infos,
        ACTION_KIND_CPP_LINK,
    )
    shared_link_flags = common_link_flags + _compute_build_flags_for_cc_action(
        default_flags_set.cxx_shared_library_infos,
        ACTION_KIND_CPP_LINK,
    )
    shared_link_actions = [
        ACTION_NAMES.cpp_link_dynamic_library,
        ACTION_NAMES.cpp_link_nodeps_dynamic_library,
    ]

    exec_link_flags = common_link_flags + _compute_build_flags_for_cc_action(
        default_flags_set.cxx_executable_infos,
        ACTION_KIND_CPP_LINK,
    )
    exec_link_actions = [
        ACTION_NAMES.cpp_link_executable,
    ]

    flag_sets = []
    if conly_compile_flags:
        flag_sets.append(define_flag_set(conly_compile_actions, conly_compile_flags))
    if cxx_compile_flags:
        flag_sets.append(define_flag_set(cxx_compile_actions, cxx_compile_flags))
    if shared_link_flags:
        flag_sets.append(define_flag_set(shared_link_actions, shared_link_flags))
    if exec_link_flags:
        flag_sets.append(define_flag_set(exec_link_actions, exec_link_flags))

    return feature(
        name = BUILD_FLAGS_FEATURE_NAME,
        enabled = True,
        flag_sets = flag_sets,
    )
