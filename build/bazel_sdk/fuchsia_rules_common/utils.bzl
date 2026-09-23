# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""General utility Starlark functions for Fuchsia Bazel rules."""

load("@rules_cc//cc/common:cc_common.bzl", "cc_common")

def fuchsia_cpu_from_ctx(ctx):
    """ Returns the Fuchsia CPU for the given rule invocation.

    Args:
        ctx: The rule context for which to find a toolchain.
    Returns:
        The Fuchsia CPU string.
    """
    target_cpu = ctx.var["TARGET_CPU"]
    if target_cpu == "aarch64":
        return "arm64"
    return target_cpu

def flatten(elements):
    # buildifier: disable=function-docstring-args
    # buildifier: disable=function-docstring-return
    """Flattens an arbitrarily nested list of lists to non-list elements while preserving order."""
    result = []
    unprocessed = list(elements)
    for _ in range(len(str(unprocessed))):
        if not unprocessed:
            return result
        elem = unprocessed.pop(0)
        if type(elem) in ("list", "tuple"):
            unprocessed = list(elem) + unprocessed
        else:
            result.append(elem)
    fail("Unable to flatten list!")

def stub_executable(ctx):
    # buildifier: disable=function-docstring-args
    # buildifier: disable=function-docstring-return
    """Returns a stub executable that fails with a message."""
    executable_file = ctx.actions.declare_file(ctx.label.name + "_fail.sh")
    content = """#!/bin/bash
    echo "---------------------------------------------------------"
    echo "ERROR: Attempting to run a target or dependency that is not runnable"
    echo "Got {target}"
    echo "---------------------------------------------------------"
    exit 1
    """.format(target = ctx.attr.name)

    ctx.actions.write(
        output = executable_file,
        content = content,
        is_executable = True,
    )

    return executable_file

def get_target_deps_from_attributes(rule_attr, rule_kind = None, known_rule_kinds = {}):
    """Return all dependencies from a given target context during analysis.

    Args:
        rule_attr: The ctx.attr value for the current target.
        rule_kind: Optional string for the rule kind (this is aspect_ctx.rule.kind
             when called from an aspect implementation function). If provided,
             this can speed up the computation for a few known target kinds.
        known_rule_kinds: Optional dictionary containing known rule kinds and their attributes to check
    Returns:
        A list of Target values corresponding to the dependencies of the current
        target.
    """
    attr_names = known_rule_kinds.get(rule_kind)
    if not attr_names:
        # For unknown rule kinds, parse all attributes and filter
        # those that are Targets or lists of Targets to the result.
        attr_names = dir(rule_attr)

    result = []
    for attr_name in attr_names:
        attr_value = getattr(rule_attr, attr_name, None)
        if not attr_value:
            continue
        if type(attr_value) == "Target":
            result.append(attr_value)
            continue
        if type(attr_value) == "list" and len(attr_value) > 0 and type(attr_value[0]) == "Target":
            result.extend(attr_value)
            continue

    return depset(result).to_list()

def make_resource_struct(src, dest):
    """Make src->dest resource mapping structure

    Args:
        src: The source path (local file path).
        dest: The destination path (ie, in a package).

    Returns:
        A struct of the mapping.
    """
    return struct(
        src = src,
        dest = dest,
    )

#TODO(b/341799247) The logic for find_cc_toolchain is copied from
#https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/bazel_rules_cc/cc/find_cc_toolchain.bzl;l=56;drc=13d212d39bbc415fd971138396cfd99320e04517
#
# We need to do this because we are currently using an older version of rules_cc
# that is not compatible with our current version of bazel. Once we roll rules_cc
# we can go back to using the method defined there.
CC_TOOLCHAIN_TYPE = "@bazel_tools//tools/cpp:toolchain_type"

def find_cc_toolchain(ctx):
    """Returns the current `CcToolchainInfo`.

    Args:
      ctx: The rule context for which to find a toolchain.

    Returns:
      A CcToolchainInfo.
    """

    # Check the incompatible flag for toolchain resolution.
    if hasattr(cc_common, "is_cc_toolchain_resolution_enabled_do_not_use") and cc_common.is_cc_toolchain_resolution_enabled_do_not_use(ctx = ctx):
        if not CC_TOOLCHAIN_TYPE in ctx.toolchains:
            fail("In order to use find_cc_toolchain, your rule has to depend on C++ toolchain. See find_cc_toolchain.bzl docs for details.")
        toolchain_info = ctx.toolchains[CC_TOOLCHAIN_TYPE]
        if toolchain_info == None:
            # No cpp toolchain was found, so report an error.
            fail("Unable to find a CC toolchain using toolchain resolution. Target: %s, Platform: %s, Exec platform: %s" %
                 (ctx.label, ctx.fragments.platform.platform, ctx.fragments.platform.host_platform))
        if hasattr(toolchain_info, "cc_provider_in_toolchain") and hasattr(toolchain_info, "cc"):
            return toolchain_info.cc
        return toolchain_info

    # Fall back to the legacy implicit attribute lookup.
    if hasattr(ctx.attr, "_cc_toolchain"):
        return ctx.attr._cc_toolchain[cc_common.CcToolchainInfo]

    # We didn't find anything.
    fail("In order to use find_cc_toolchain, your rule has to depend on C++ toolchain. See find_cc_toolchain.bzl docs for details.")
