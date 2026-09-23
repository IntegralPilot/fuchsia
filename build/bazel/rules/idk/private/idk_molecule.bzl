# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines an IDK molecule."""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@fuchsia_build_info//:args.bzl", "target_cpu")
load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
load("//build/bazel/rules:current_platform_info.bzl", "CurrentPlatformInfo")
load(
    ":idk_transitions.bzl",
    "api_level_and_cpu_combinations_transition",
    "configured_host_cpus_transition",
    "current_host_cpu_transition",
)
load(
    ":providers.bzl",
    "FuchsiaIdkAtomInfo",
    "FuchsiaIdkMoleculeInfo",
)

visibility([
    "//build/bazel/rules/idk/...",
    "//sdk",
])

# IDK molecules may be one of the following:
# * Contains Fuchsia or OS-independent molecules and/or atoms and is
#   `target_compatible_with` `["@platforms//os:fuchsia"]`.
# * Contains host tool molecules and/or atoms and is `target_compatible_with`
#   `HOST_OS_CONSTRAINTS`. Their `allowed_in_configurations` attribute must be
#   `"PLATFORM"`.
# * A special molecule that `target_compatible_with`
#   `["@platforms//os:fuchsia"]` but contains host tool molecules and/or atoms
#   that are `target_compatible_with` `HOST_OS_CONSTRAINTS`. Such molecules use
#   a transition to build the host tools in the appropriate configuration(s).

def _verify_target_compatible_with_is_fuchsia(ctx):
    if (len(ctx.attr.target_compatible_with) != 1 or
        str(ctx.attr.target_compatible_with[0].label) != "@@platforms//os:fuchsia"):
        fail('`target_compatible_with` must be `["@platforms//os:fuchsia"]`.')

def _idk_molecule_common_impl(ctx, allowed_in_configurations):
    if not ctx.attr.name.endswith("_idk"):
        fail("IDK molecule `name`s must end with `_idk`.")

    if (len(ctx.attr.target_compatible_with) != 1):
        fail("`target_compatible_with` must have exactly one element, not `%s`." % ctx.attr.target_compatible_with)
    elif str(ctx.attr.target_compatible_with[0].label) == "@@platforms//os:fuchsia":
        pass
    elif str(ctx.attr.target_compatible_with[0].label) == ("@" + HOST_OS_CONSTRAINTS[0]):
        if not ctx.attr.allowed_in_configurations == "PLATFORM":
            fail('`allowed_in_configurations` must be "PLATFORM" when `target_compatible_with` is `HOST_OS_CONSTRAINTS`.')
    else:
        fail('`target_compatible_with` must be `["@platforms//os:fuchsia"]` or `HOST_OS_CONSTRAINTS`.')

    if allowed_in_configurations != "fuchsia":
        api_level = ctx.attr._current_api_level[BuildSettingInfo].value
        if api_level != "PLATFORM":
            fail('This molecule is only to be built at the "PLATFORM" API level, not "%s".' %
                 api_level)
    if allowed_in_configurations == "once":
        current_cpu = ctx.attr._current_platform[CurrentPlatformInfo].cpu
        if current_cpu != target_cpu:
            fail('This molecule is only to be built for the target CPU architecture ("%s"), not "%s".' %
                 (target_cpu, current_cpu))

    all_deps_depset = depset(direct = ctx.files.deps)

    # Build the atoms depset, excluding molecules while including their atoms.
    direct_deps = []
    transitive_depsets = []
    for dep in ctx.attr.deps:
        if FuchsiaIdkAtomInfo in dep:
            direct_deps.append(dep)
            transitive_depsets.append(dep[FuchsiaIdkAtomInfo].atoms_depset)
        elif FuchsiaIdkMoleculeInfo in dep:
            transitive_depsets.append(dep[FuchsiaIdkMoleculeInfo].atoms_depset)
        else:
            fail("Unexpected dependency %s. Must be an atom or a molecule." % dep)

    atoms_depset = depset(direct = direct_deps, transitive = transitive_depsets)

    return [
        DefaultInfo(files = all_deps_depset),
        FuchsiaIdkMoleculeInfo(
            label = ctx.label,
            deps = ctx.attr.deps,
            atoms_depset = atoms_depset,
        ),
    ]

COMMON_MOLECULE_ATTRS = {
    "_current_api_level": attr.label(
        default = "@//build/bazel/versioning:api_level",
    ),
    "_current_platform": attr.label(
        providers = [CurrentPlatformInfo],
        default = "@//build/bazel:current_platform",
    ),
}

def _idk_molecule_impl(ctx):
    return _idk_molecule_common_impl(ctx, ctx.attr.allowed_in_configurations)

idk_molecule = rule(
    doc = """Generate an IDK molecule containing atoms for Fuchsia targets.

    * `name` must end with '_idk' (unlike most other IDK macros).
    * `target_compatible_with` must be `["@platforms//os:fuchsia"]` or
      `HOST_OS_CONSTRAINTS`.
    """,
    implementation = _idk_molecule_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Atoms and other molecules the molecule depends on.",
            providers = [[FuchsiaIdkAtomInfo], [FuchsiaIdkMoleculeInfo]],
            mandatory = True,
        ),
        "allowed_in_configurations": attr.string(
            doc = """The configurations in which this molecule is allowed to be built.

            * "fuchsia": The molecule can be built in any Fuchsia configuration,
              including the main "PLATFORM" build, any API level, and any CPU
              architecture.
            * "once": The molecule can only be built in one configuration, the
              main "PLATFORM" build.
            * "PLATFORM": The molecule can only be built at the "PLATFORM"
              API level, in any CPU architecture.
            """,
            values = ["fuchsia", "once", "PLATFORM"],
            mandatory = True,
        ),
    } | COMMON_MOLECULE_ATTRS,
)

def _idk_all_api_level_and_cpu_combinations_molecule_impl(ctx):
    _verify_target_compatible_with_is_fuchsia(ctx)
    return _idk_molecule_common_impl(ctx, allowed_in_configurations = "once")

idk_all_api_level_and_cpu_combinations_molecule = rule(
    doc = """Generate an IDK molecule containing molecules for each combination of IDK buildable API level and CPU architecture.

    Each molecule in `deps` will be replicated for each combination of API level
    and CPU architecture supported by the IDK build.

    * `name` must end with '_idk' (unlike most other IDK macros).
    * `target_compatible_with` must be `["@platforms//os:fuchsia"]`.

    Only supported in the main "PLATFORM" Fuhsia build.
    """,
    implementation = _idk_all_api_level_and_cpu_combinations_molecule_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Molecules to be replicated for each combination of IDK buildable API level and CPU architecture.",
            providers = [FuchsiaIdkMoleculeInfo],
            mandatory = True,
            cfg = api_level_and_cpu_combinations_transition,
        ),
    } | COMMON_MOLECULE_ATTRS,
)

def _idk_host_tool_molecule_for_host_cpus_impl(ctx):
    _verify_target_compatible_with_is_fuchsia(ctx)
    return _idk_molecule_common_impl(ctx, allowed_in_configurations = "once")

idk_host_tool_molecule_for_current_host_cpu = rule(
    doc = """Generate an IDK molecule containing atoms for the current host OS and CPU architecture.

    * `name` must end with '_idk' (unlike most other IDK macros).
    * `target_compatible_with` must be `["@platforms//os:fuchsia"]`.
      The `deps` will be built for the current host platform via a transition.
    """,
    implementation = _idk_host_tool_molecule_for_host_cpus_impl,
    attrs = {
        "deps": attr.label_list(
            doc = """Atoms and other molecules the molecule depends on.
            All must be `target_compatible_with` `HOST_OS_CONSTRAINTS` or
            `HOST_CONSTRAINTS`.
            """,
            providers = [[FuchsiaIdkAtomInfo], [FuchsiaIdkMoleculeInfo]],
            mandatory = True,
            cfg = current_host_cpu_transition,
        ),
    } | COMMON_MOLECULE_ATTRS,
)

idk_host_tool_molecule_for_configured_host_cpus = rule(
    doc = """Generate an IDK molecule containing atoms for the current host OS and configured host CPU architectures.

    * `name` must end with '_idk' (unlike most other IDK macros).
    * `target_compatible_with` must be `["@platforms//os:fuchsia"]`.
      The `deps` will be built for the current host OS and configured host CPU
      architectures via a transition.
    """,
    implementation = _idk_host_tool_molecule_for_host_cpus_impl,
    attrs = {
        "deps": attr.label_list(
            doc = """Atoms and other molecules the molecule depends on.
            All must be `target_compatible_with` `HOST_OS_CONSTRAINTS`.
            """,
            providers = [[FuchsiaIdkAtomInfo], [FuchsiaIdkMoleculeInfo]],
            mandatory = True,
            cfg = configured_host_cpus_transition,
        ),
    } | COMMON_MOLECULE_ATTRS,
)
