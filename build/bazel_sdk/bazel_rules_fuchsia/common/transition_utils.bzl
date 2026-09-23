# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities for transition functions."""

def set_command_line_option_value(input_args, option_prefix, option_value):
    """Set or reset of command-line option such as "--foo=bar".

    Args:
        input_args: [list(string)] A input list of command arguments.
        option_prefix: [string] The option prefix (e.g. "--foo=")
        option_value: [string] The new option value (e.g. "bar").
    Returns:
        If the option already appears in the input, change its value to option_value

        If multiple instances of the option appear in the input, they are all
        changed to the new value.

        Otherwise (i.e. if the option prefix is not in the input), return the input list
        with "${option_prefix}${option_value}" appended to it.
    """
    new_arg = option_prefix + option_value
    result = []
    replaced = False
    for arg in input_args:
        if arg.startswith(option_prefix):
            arg = new_arg
            replaced = True
        result.append(arg)

    if not replaced:
        result.append(new_arg)

    return result

def remove_command_line_option_values(input_args, option_prefix):
    """Remove all instances of a command-line option from an input arguments list.

    Args:
        input_args: [list(string)] A input list of command arguments.
        option_prefix: [string] The option prefix (e.g. "--foo=").
    Returns:
        new argument list, with all argument beginning with option_prefix removed.
    """
    return [arg for arg in input_args if not arg.startswith(option_prefix)]

def normalize_features_list(features):
    """Normalize a list of feature flag names.

    Args:
        features: A string list, each item is a feature flag name, that can be
            optionally prefixed with '-' to disable it, instead of enabling it.

    Returns:
        A string list that contains a normalized version of the input list.
        This means that:
          - Duplicates are removed from the output.
            E.g. [ "foo", "foo" ] --> [ "foo" ]

          - For a given feature flag name, only the last definition wins,
            E.g. [ "foo", "-foo" ] --> [ "-foo" ]
                 [ "-foo", "foo" ] --> [ "foo" ]

         - The last definition also keeps its place / order in the final result.
           E.g. [ "foo", "bar", "-foo" ] -> [ "bar", "-foo" ]
    """
    features_map = {}

    # To ensure that the last definition wins while keeping its order,
    # parse the features in  reversed order when building the features_map
    # dictionary (which maintains insertion order of its keys). Then reverse
    # the values in the final list.
    for feature in reversed(features):
        key = feature
        if feature[0] == "-":
            key = feature[1:]
        features_map.setdefault(key, feature)

    return reversed(features_map.values())

def _features_list_transition_impl(settings, attr):
    cur_features = settings["//command_line_option:features"]
    new_features = normalize_features_list(cur_features + attr.feature_names)
    return {"//command_line_option:features": new_features}

FEATURES_LIST_TRANSITION_ATTRS = {
    # NOTE: The "features" attribute name is built-in and reserved by Bazel.
    "feature_names": attr.string_list(
        mandatory = True,
        doc = "A list of feature flag names, each item can be prefixed with '-' to disable it instead of enabling it.",
    ),
    "_allowlist_function_transition": attr.label(
        default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
    ),
}

features_list_transition = transition(
    implementation = _features_list_transition_impl,
    inputs = ["//command_line_option:features"],
    outputs = ["//command_line_option:features"],
)

def _apply_features_list_impl(ctx):
    return [
        ctx.attr.actual[DefaultInfo],
    ]

apply_features_list = rule(
    implementation = _apply_features_list_impl,
    attrs = {
        "actual": attr.label(
            mandatory = True,
            doc = "Label to actual target to build under the new build configuration.",
            cfg = features_list_transition,
        ),
    } | FEATURES_LIST_TRANSITION_ATTRS,
)
