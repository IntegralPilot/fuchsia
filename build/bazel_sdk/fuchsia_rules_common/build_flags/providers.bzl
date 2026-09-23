# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Providers for the build_flags() feature."""

#############################################################################
#############################################################################
#####
#####    BuildFlagsInfo
#####

_BUILD_FLAGS_INFO_DOC = """A provider to store compiler and linker flags for C++ and Rust.

Similar to GN configs, except for subtle differences documented in
//build/bazel_sdk/fuchsia_rules_common/build_flags/README.md.

Note also that Bazel targets cannot set 'all_dependent_configs' and 'public_configs', as these
modify the build graphs in ways that Bazel doesn't support. However, most GN use cases are covered
by using Bazel builtin attributes, such as 'defines' or 'includes' in 'cc_library()' which apply to
the target and all its dependents (unlike 'local_defines').

For the other rare cases where these are used in the GN graph, a Bazel-specific solution is
required to implement the same feature.
"""

def _build_flags_info_init(
        *,
        label,
        defines = [],
        cflags = [],
        cflags_c = [],
        cflags_cc = [],
        include_dirs = [],
        ldflags = [],
        lib_dirs = [],
        rustenv = [],
        rustflags = []):
    # Consistency check for rustenv list.
    for item in rustenv:
        equal_pos = item.find("=")
        if equal_pos < 1:
            fail("Invalid rustenv item {}, must follow NAME=value format".format(item))

    return {
        "label": label,
        "defines": defines,
        "cflags": cflags,
        "cflags_c": cflags_c,
        "cflags_cc": cflags_cc,
        "include_dirs": include_dirs,
        "ldflags": ldflags,
        "lib_dirs": lib_dirs,
        "rustenv": rustenv,
        "rustflags": rustflags,
    }

BuildFlagsInfo, _ = provider(
    doc = _BUILD_FLAGS_INFO_DOC,
    # LINT.IfChange(BuildFlagsInfo)
    fields = {
        "label": "(Label) The canonical Label of the build_flags() rule, used for deduplication and debugging.",
        "defines": "(list[string]) A list of macro definitions for C and C++ compile actions (e.g ['FOO=1']).",
        "cflags": "(list[string]) A list of C and C++ compiler flags.",
        "cflags_c": "(list[string]) A list of C compiler flags.",
        "cflags_cc": "(list[string]) A list of C++ compiler flags.",
        "include_dirs": "(list[string]) A list of include directories, relative to the workspace root.",
        "ldflags": "(list[string]) A list of linker flags.",
        "lib_dirs": "(list[string]) A list of library search directories, relative to the workspace root.",
        "rustenv": "(list[string]) A list of strings in the format 'VARNAME=VARVALUE'.",
        "rustflags": "(list[string]) A list of Rust compiler flags.",
    },
    # LINT.ThenChange(
    #    //build/bazel/starlark/expand_build_args.cquery:BuildFlagsInfo,
    #    //build/bazel/starlark/expand_build_args_json.cquery:BuildFlagsInfo,
    # )
    init = _build_flags_info_init,
)

#############################################################################
#############################################################################
#####
#####    BuildFlagsListInfo
#####

BuildFlagsListInfo = provider(
    doc = "A provider for a list of BuildFlagsInfo values.",
    fields = {
        "infos": "A list of BuildFlagsInfo values.",
    },
)

#############################################################################
#############################################################################
#####
#####    DefaultBuildFlagsSetInfo
#####

# LINT.IfChange(DefaultBuildFlagsSetInfo)
DefaultBuildFlagsSetInfo = provider(
    doc = "A set of default build_flags() targets for different target types.",
    fields = {
        "cxx_common_infos": """
            (list[BuildFlagsInfo]) list of build flags common to all C/C++ target types
            (libraries, shared libraries and executables).""",
        "cxx_shared_library_infos": """
            (list[BuildFlagsInfo]) list of extra build flags used for C/C++ shared libraries.""",
        "cxx_executable_infos": """
            (list[BuildFlagsInfo]) list of extra build flags used for C/C++ executables.""",
        "rust_common_infos": """
            (list[BuildFlagsInfo]) list of build flags common to all Rust target types
            (libraries, shared libraries and executables).""",
        "rust_shared_library_infos": """
            (list[BuildFlagsInfo]) list of extra build flags used for Rust shared libraries.""",
        "rust_executable_infos": """
            (list[BuildFlagsInfo]) list of extra build flags used for Rust executables.""",
    },
)
# LINT.ThenChange(//build/bazel/scripts/bazel_build_flags.py:DefaultBuildFlagsSet)
