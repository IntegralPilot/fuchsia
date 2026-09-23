# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("//build/bazel/rules:current_platform_info.bzl", "CurrentPlatformInfo")
load("//build/bazel/rules/cc:providers.bzl", "PrebuiltLibraryInfo")
load("//build/bazel/rules/idk:providers.bzl", "FuchsiaIdkAtomInfo")

def _get_current_cpu_arch(ctx):
    """Returns the CPU architecture of the current build."""
    current_platform = ctx.attr._current_platform[CurrentPlatformInfo]
    return current_platform.cpu

# LINT.IfChange(idk_atom_info)
def _create_test_atom_info_impl(ctx):
    # In the actual atom info:
    # * Labels for source files are identified as `<input file target>`
    # * Labels for binaries are identified as `<generated file>`
    # If we just use the label that generates the file, we will get a `<target>`
    # with that label. If we get the file from `DefaultInfo`, it is a
    # `<generated file>`. However, if we use `DefaultInfo` for a source file, it
    # is a `<source file>`. Thus, we must use two different methods for these
    # two cases, matching the implementation.
    # The following code merges the two dictionaries using the appropriate
    # mechanism for each type of file.
    atom_files_map = dict(ctx.attr.atom_files_map_source_files)
    for dest_path, source_file in ctx.attr.atom_files_map_generated_files.items():
        atom_files_map[dest_path] = source_file[DefaultInfo].files.to_list()[0]

    return [
        FuchsiaIdkAtomInfo(
            label = ctx.attr.label.label,
            idk_name = ctx.attr.idk_name,
            id = ctx.attr.id,
            meta_dest = ctx.attr.meta_dest,
            type = ctx.attr.type,
            category = ctx.attr.category,
            is_stable = ctx.attr.is_stable,
            api_area = ctx.attr.api_area,
            api_file_path = ctx.attr.api_file_path,
            api_contents_map = ctx.attr.api_contents_map,
            atom_files_map = atom_files_map,
            deps = ctx.attr.deps,
            atoms_depset = depset(ctx.attr.atoms_depset_as_list),
            atom_build_deps = ctx.attr.atom_build_deps,
            additional_prebuild_info = ctx.attr.additional_prebuild_info,
        ),
    ]

create_test_atom_info = rule(
    doc = "Creates a FuchsiaIdkAtomInfo provider for use with `verify_atom_info()`." +
          "For CPU architecture-specific strings, use `x64` as a placeholder " +
          "for the current CPU architecture. `verify_atom_info()` will " +
          "replace all instances of `x64` with the current CPU architecture " +
          "before comparing the expected and actual atom info.",
    implementation = _create_test_atom_info_impl,
    attrs = {
        "label": attr.label(mandatory = True),
        "idk_name": attr.string(mandatory = True),
        "id": attr.string(mandatory = True),
        "meta_dest": attr.string(mandatory = True),
        "type": attr.string(mandatory = True),
        "category": attr.string(mandatory = True),
        "is_stable": attr.bool(mandatory = True),
        "api_area": attr.string(mandatory = True),
        "api_file_path": attr.label(mandatory = False, allow_single_file = True),
        "api_contents_map": attr.string_keyed_label_dict(mandatory = True, allow_files = True),
        "atom_files_map_source_files": attr.string_keyed_label_dict(
            doc = "Mappings in `atom_files_map` for static source files.",
            mandatory = True,
            allow_files = True,
        ),
        "atom_files_map_generated_files": attr.string_keyed_label_dict(
            doc = "Mappings in `atom_files_map` for generated files. Use the " +
                  "label that generates the file and returns it as the first " +
                  "element of `DefaultInfo.files`.",
            mandatory = True,
            allow_files = True,
        ),
        "deps": attr.label_list(mandatory = True),
        "atom_build_deps": attr.label_list(mandatory = True),
        "additional_prebuild_info": attr.string_dict(mandatory = True),
        "atoms_depset_as_list": attr.label_list(
            doc = "List of atoms to include in the `atoms_depset`. " +
                  "This is necessary since a depset cannot be passed as an attribute.",
            mandatory = True,
        ),
    },
)
# LINT.ThenChange(//build/bazel/rules/idk/private/providers.bzl:idk_atom_info)

def _verify_atom_info_impl(ctx):
    atom_info = ctx.attr.atom[FuchsiaIdkAtomInfo]
    atom_info_string = str(atom_info)

    expected_atom_info = ctx.attr.expected_atom_info[FuchsiaIdkAtomInfo]

    # Replace instances of `x64` with the current CPU architecture after
    # converting the expected info into a string. This is the simplest way to
    # support these tests on multiple CPU architectures. It avoids complicating
    # each test definition with `select()` statements or iterating through maps
    # in `create_test_atom_info()`.
    expected_atom_info_string = str(expected_atom_info).replace("x64", _get_current_cpu_arch(ctx))

    # Compare string representations because the objects are not equal.
    if atom_info_string != expected_atom_info_string:
        # Break the strings on commas to make the output more readable and easy
        # to copy to a diff viewer.
        fail("The actual atom info does not match the expected atom info.\nActual:\n%s\nExpected:\n%s\n" % (
            atom_info_string.replace(",", ",\n"),
            expected_atom_info_string.replace(",", ",\n"),
        ))
    return []

verify_atom_info = rule(
    doc = "Verifies that the actual FuchsiaIdkAtomInfo provider of an atom " +
          "target matches the expected provider representations for testing. " +
          "Replaces instances of `x64` in `expected_atom_info` with the current CPU architecture.",
    implementation = _verify_atom_info_impl,
    attrs = {
        "atom": attr.label(mandatory = True, providers = [FuchsiaIdkAtomInfo]),
        "expected_atom_info": attr.label(mandatory = True, providers = [FuchsiaIdkAtomInfo]),
        "_current_platform": attr.label(
            providers = [CurrentPlatformInfo],
            default = "@//build/bazel:current_platform",
        ),
    },
)

def _get_debug_file_impl(ctx):
    debug = ctx.attr.prebuilt_library[PrebuiltLibraryInfo].debug
    if not debug:
        fail("`debug` must not be empty")
    return [DefaultInfo(files = depset([debug]))]

get_debug_file = rule(
    doc = "Simply declares the debug file from the `PrebuiltLibraryInfo` " +
          "provider as an output. This allows tests to access the file.",
    implementation = _get_debug_file_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            allow_files = False,
            providers = [PrebuiltLibraryInfo],
        ),
    },
)

def _get_stripped_file_impl(ctx):
    stripped = ctx.attr.prebuilt_library[PrebuiltLibraryInfo].stripped
    if not stripped:
        fail("`stripped` must not be empty")
    return [DefaultInfo(files = depset([stripped]))]

get_stripped_file = rule(
    doc = "Simply declares the `stripped` file from the `PrebuiltLibraryInfo` " +
          "provider as an output. This allows tests to access the file.",
    implementation = _get_stripped_file_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            allow_files = False,
            providers = [PrebuiltLibraryInfo],
        ),
    },
)

def _get_link_lib_file_impl(ctx):
    link_lib = ctx.attr.prebuilt_library[PrebuiltLibraryInfo].link_lib
    if not link_lib:
        fail("`link_lib` must not be empty")
    return [DefaultInfo(files = depset([link_lib]))]

get_link_lib_file = rule(
    doc = "Simply declares the `link_lib` file from the `PrebuiltLibraryInfo` " +
          "provider as an output. This allows tests to access the file.",
    implementation = _get_link_lib_file_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            allow_files = False,
            providers = [PrebuiltLibraryInfo],
        ),
    },
)

def _get_stripped_ifs_file_impl(ctx):
    ifs_file = ctx.attr.prebuilt_library[PrebuiltLibraryInfo].stripped_ifs_file
    if not ifs_file:
        fail("`stripped_ifs_file` must not be empty")
    return [DefaultInfo(files = depset([ifs_file]))]

get_stripped_ifs_file = rule(
    doc = "Simply declares the `stripped_ifs_file` from the `PrebuiltLibraryInfo` " +
          "provider as an output. This allows tests to access the file.",
    implementation = _get_stripped_ifs_file_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            allow_files = False,
            providers = [PrebuiltLibraryInfo],
        ),
    },
)
