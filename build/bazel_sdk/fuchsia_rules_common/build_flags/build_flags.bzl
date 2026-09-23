# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Implementation of the build_flags() rule and related definitions.

See README.md file for full technical details.
"""

load(":providers.bzl", "BuildFlagsInfo")

#############################################################################
#############################################################################
#####
#####    build_flags() rule
#####

def _build_flags_impl(ctx):
    cflags = list(ctx.attr.cflags)
    cflags_c = list(ctx.attr.cflags_c)
    cflags_cc = list(ctx.attr.cflags_cc)
    defines = list(ctx.attr.defines)
    include_dirs = list(ctx.attr.include_dirs)
    ldflags = list(ctx.attr.ldflags)
    lib_dirs = list(ctx.attr.lib_dirs)
    rustenv = list(ctx.attr.rustenv)
    rustflags = list(ctx.attr.rustflags)

    # Similar to what GN does with the `configs` argument, `subflags`
    # appends the sub-flags directly to the current item's flags.

    # Hence, there is no way for targets to disable a sub-flag by listing
    # it in disable_build_flags (just like there is no way to remove a sub-config)
    # label from the `configs` list that only includes a label to the parent
    # config()).
    for subtarget in ctx.attr.subflags:
        info = subtarget[BuildFlagsInfo]
        cflags.extend(info.cflags)
        cflags_c.extend(info.cflags_c)
        cflags_cc.extend(info.cflags_cc)
        defines.extend(info.defines)
        include_dirs.extend(info.include_dirs)
        ldflags.extend(info.ldflags)
        lib_dirs.extend(info.lib_dirs)
        rustenv.extend(info.rustenv)
        rustflags.extend(info.rustflags)

    return [
        BuildFlagsInfo(
            label = ctx.label,
            cflags = cflags,
            cflags_c = cflags_c,
            cflags_cc = cflags_cc,
            defines = defines,
            include_dirs = include_dirs,
            ldflags = ldflags,
            lib_dirs = lib_dirs,
            rustenv = rustenv,
            rustflags = rustflags,
        ),
    ]

build_flags = rule(
    doc = "Define a target exposing toolchain build flags to its dependents.",
    implementation = _build_flags_impl,
    provides = [BuildFlagsInfo],
    attrs = {
        "cflags": attr.string_list(
            doc = "List of C and C++ compiler flags.",
            default = [],
        ),
        "cflags_c": attr.string_list(
            doc = "List of C only compiler flags. Always appear after 'cflags' on the command-line.",
            default = [],
        ),
        "cflags_cc": attr.string_list(
            doc = "List of C++ only compiler flags. Always appear after 'cflags' on the command-line.",
            default = [],
        ),
        "defines": attr.string_list(
            doc = "List of C and C++ macro definitions for compile actions",
            default = [],
        ),
        "include_dirs": attr.string_list(
            doc = "List of header search paths, must be relative to the workspace, not the package",
            default = [],
        ),
        "ldflags": attr.string_list(
            doc = "List of linker flags.",
            default = [],
        ),
        "lib_dirs": attr.string_list(
            doc = "List of library search paths, relative to the workspace root.",
            default = [],
        ),
        "rustenv": attr.string_list(
            doc = "List of Rust environment variable definitions, each item should be a 'NAME=value' string",
            default = [],
        ),
        "rustflags": attr.string_list(
            doc = "List of Rust compiler flags.",
            default = [],
        ),
        "subflags": attr.label_list(
            doc = "List of other build_flags() targets whose flags will be appended to this rule's flags.",
            providers = [BuildFlagsInfo],
            default = [],
        ),
    },
)

#############################################################################
#############################################################################
#####
#####    compute_final_build_flags_from()
#####

def compute_final_build_flags_from(build_flags_infos, disabled_build_flags_labels):
    """Compute the final ordered list of BuildFlags for a given target.

    Note that filtering and deduplication happen at the label level, e.g.
    this does not remove duplicate flags in a given BuildFlagsInfo definition.

    Args:
      build_flags_infos: A list of `BuildFlagsInfo` from a 'build_flags' target
        attribute.
      disabled_build_flags_labels: A list of labels to `disabled build_flags`
        targets.
    Returns:
      The input list, without duplicates, and without items whose label is in
      `disable_build_flags_labels`.
    """

    # Remove duplicate labels. First label wins, so
    # ["//foo", "//bar", "//foo"] is equivalent to ["//foo", "//bar"]
    # and *not* ["//bar", "//foo"]. This is similar to GN.
    known_labels = set(disabled_build_flags_labels)
    result = []
    for info in build_flags_infos:
        if info.label not in known_labels:
            known_labels.add(info.label)
            result.append(info)
    return result
