# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines an IDK C/C++ prebuilt library."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@rules_cc//cc:cc_static_library.bzl", "cc_static_library")
load("@rules_cc//cc:defs.bzl", "cc_import", "cc_library", "cc_shared_library")
load("//build/bazel/rules:zx_library.bzl", "apply_common_zx_library_modifications")
load(
    "//build/bazel/rules/cc:shared_library.bzl",
    "generate_companion_files_for_shared_library",
    "get_library_info_for_static_library",
    "verify_public_symbols",
)
load(
    ":cc_verification.bzl",
    "create_verify_no_duplicate_files_target",
    "create_verify_pragma_once_target",
)
load(":idk_atom.bzl", "idk_atom")
load(
    ":idk_common.bzl",
    "get_atom_visibility",
    "get_idk_deps",
    "json_encode_dict_values",
    "select_for_fuchsia",
    "verify_target_is_in_allowlist",
)

visibility([
    "//build/bazel/rules/idk/...",
])

# LINT.IfChange(idk_cc_prebuilt_library)

def get_shared_library_output_name(name, output_name):
    """Returns the output name for a shared library given the target name and output name attributes.

    Args:
        name: The target name.
        output_name: The specified output name.
    Returns:
        The output name.
    """
    if output_name == "":
        return name
    elif output_name == name:
        fail("The specified `output_name` (`%s`) matches the default. `output_name` only needs to be specified when overriding the default." % output_name)
    else:
        return output_name

def get_ifs_golden_file_name(output_name):
    return "lib" + output_name + ".ifs"

def _idk_cc_prebuilt_library_impl(
        name,
        prebuilt_library_type,
        idk_name,
        category,
        stable,
        api_area,
        hdrs,
        hdrs_for_internal_use,
        srcs,
        deps,
        fuchsia_deps,
        implementation_deps,
        runtime_deps,
        include_base,
        api_file_path,
        output_name,
        no_headers,
        libcxx_linkage,
        ifs_golden_file,
        testonly,
        visibility,
        friend,  # buildifier: disable=unused-variable - For GN conversion only.
        public_configs,  # buildifier: disable=unused-variable - For GN conversion only.
        ldflags = [],  # buildifier: disable=unused-variable - For GN conversion only.
        inputs = [],  # buildifier: disable=unused-variable - For GN conversion only.
        version_script = "",
        **kwargs):
    """Implementation for the _idk_cc_prebuilt_library() macro."""

    if "data" in kwargs:
        fail("Use `runtime_deps` instead of `data` for atoms that are runtime dependencies.")

    # TODO(https://fxbug.dev/428229472): When migrating "vfs_internal", it may
    # be necessary to add exceptions to these two checks for empty `hdrs`.
    if not hdrs and not no_headers:
        fail("`hdrs` cannot be empty unless `no_headers` is True.")

    if hdrs_for_internal_use and not hdrs:
        fail("`hdrs_for_internal_use` must be empty when `hdrs` is empty.")

    if no_headers and prebuilt_library_type != "shared":
        fail("`no_headers` is only supported for 'shared' libraries.")

    if no_headers and (hdrs != [] or deps != []):
        fail("There must be no public headers or dependencies when `no_headers` is True.")

    if ifs_golden_file and prebuilt_library_type != "shared":
        fail("`ifs_golden_file` is only supported for 'shared' libraries.")

    if runtime_deps:
        if implementation_deps == []:
            fail("Runtime dependencies are only applicable if there are private dependencies.")

        # TODO(https://fxbug.dev/447151364): Fail if any are not shared libraries.
        # That may need to be done elsewhere.

        # TODO(https://fxbug.dev/447151364): Implement support for runtime dependencies.
        # This includes the "subtle" logic mentioned below and using an aspect
        # to collect runtime dependencies for prebuild info.
        pass

    if category not in ["partner"]:
        # Other categories are only to ensure ABI compatibility and thus not
        # applicable.
        fail("Category '%s' is not supported." % category)

    if api_file_path and not stable:
        fail("Unstable targets do not require/support modification acknowledgement.")

    if api_file_path and no_headers:
        fail("Targets without public headers do not require/support modification acknowledgement.")

    # Do not allow `name` to start with "lib". For static libraries, `name` is
    # the output name and "lib" is added automatically. Apply the same
    # restriction to all library types for consistency and simplicity.
    if name.startswith("lib"):
        fail("`name` must not start with 'lib'%s." % (
            " because 'lib' will automatically be added to the library file name" if prebuilt_library_type == "static" else ""
        ))

    if prebuilt_library_type == "static":
        # The output name is always the name of the library target, `name`.

        if output_name != "":
            fail("`output_name` is not supported for static libraries.")
    else:
        if output_name == "":
            fail("`output_name` is required for shared libraries.")

        # `output_name` should not start with `lib` for consistency and simplicity.
        if output_name.startswith("lib"):
            fail("`output_name` must not start with 'lib' because 'lib' will automatically be added to the library file name.")

    if prebuilt_library_type == "static":
        # TODO(https://fxbug.dev/447151364): Implement the "subtle" logic for
        # runtime dependencies from the GN template.
        pass

    # Group the source files for various uses.
    # Per https://bazel.build/reference/be/c-cpp#cc_library.hdrs,
    # "Headers not meant to be included by a client of this library should be
    # listed in the srcs attribute instead, even if they are included by a
    # published header." Thus, `hdrs_for_internal_use` is added to `srcs` in the
    # underlying library, not `hdrs`. However, all headers must be in the IDK.
    all_source_files = hdrs + hdrs_for_internal_use + srcs
    hdrs_for_idk = hdrs + hdrs_for_internal_use
    hdrs_for_bazel_library = hdrs
    srcs_for_bazel_library = srcs + hdrs_for_internal_use

    # Prebuilt shared libraries are eligible for inclusion in the SDK. We do not
    # want to dynamically link against libc++.so because we let clients bring
    # their own toolchain, which might have a different C++ Standard Library or
    # a different C++ ABI entirely.
    if libcxx_linkage == "none":
        # Adding this linker flag keeps us honest about not committing to a
        # specific C++ ABI. If this flag is causing your library to not
        # compile, consider whether your library really ought to be in the SDK.
        # If so, consider including your library in the SDK as source rather than
        # precompiled. If you do require precompilation, you probably need to
        # find a way not to depend on dynamically linking C++ symbols because C++
        # does not have a sufficiently stable ABI for the purposes of our SDK.
        # TODO(https://fxbug.dev/421888626): Apply the equivalent of GN's
        # `//build/config/fuchsia:no_cpp_standard_library`.
        pass
    elif libcxx_linkage == "static":
        # TODO(https://fxbug.dev/421888626): Apply the equivalent of GN's
        # `//build/config/fuchsia:static_cpp_standard_library`.
        pass
    else:
        fail("`libcxx_linkage` ('%s') must be 'none' or 'static'." % libcxx_linkage)

    if prebuilt_library_type == "shared":
        # In-tree code should depend on the imported `cc_shared_library` rather
        # than the `cc_library()`, which will not be a shared library.
        # Give the `cc_library()` a different name to make the base name
        # available for the imported library.
        cc_library_name = "%s_impl" % name

        # The `cc_library()` is not a shared object. Prevent use.
        cc_library_visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"]
    elif prebuilt_library_type == "static":
        # In-tree code should depend on the `cc_library` rather than importing
        # the `cc_static_library`, which is meant to be self-contained.
        cc_library_name = name

        # In-tree code should depend on the `cc_library()`.
        cc_library_visibility = visibility
    else:
        fail("Unrecognized `prebuilt_library_type` '%s'." % prebuilt_library_type)

    # TODO(https://fxbug.dev/421888626): Apply the equivalent of GN's
    # `default_common_binary_configs` for "static" and
    # `default_shared_library_configs` for "shared" using the `copts` attribute.
    # TODO(https://fxbug.dev/421888626): Add "//build/config:sdk_extra_warnings"
    # using the `copts` attribute here too? (It isn't in GN.)
    # TODO(https://fxbug.dev/421888626): Ensure the library is built with the
    # shared library toolchain (without variants for the "shared" case). For
    # the "static" case, this will allow the static library shipped in the IDK
    # to be linked into shared libraries See https://fxbug.dev/404169865.
    cc_library(
        name = cc_library_name,
        srcs = srcs_for_bazel_library,
        hdrs = hdrs_for_bazel_library,
        deps = deps + select_for_fuchsia(fuchsia_deps),
        implementation_deps = implementation_deps,
        includes = [include_base],
        testonly = testonly,
        visibility = cc_library_visibility,
        **kwargs
    )

    # The library included in the IDK is defined by a different rule that allows
    # it to be exported.
    if prebuilt_library_type == "shared":
        # Create the exportable shared library. This generates the `.so` file
        # that will be included in the IDK.
        exported_target_name = name + "_export"

        shared_lib_name = "lib%s.so" % output_name

        user_link_flags = ["-Wl,-soname=%s" % shared_lib_name]
        additional_linker_inputs = []
        if version_script:
            user_link_flags.append("-Wl,--version-script=$(location %s)" % version_script)
            additional_linker_inputs.append(version_script)

        cc_shared_library(
            name = exported_target_name,
            shared_lib_name = "lib%s.so" % output_name,
            deps = [":%s" % cc_library_name],
            user_link_flags = user_link_flags,
            additional_linker_inputs = additional_linker_inputs,
            testonly = testonly,
            # Only the IDK atom target should depend on this target.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        )

        # The target and `.so` file above cannot be referenced from targets that
        # require a `CcInfo` provider and thus cannot be used by in-tree
        # targets. To support in-tree targets, we must import the shared library
        # using `cc_import()` and reference that target instead. To make this
        # transparent to in-tree users, the import's name is `name`.
        # This target has no impact on the IDK.

        # Bazel 8 does not automatically make the include path relative to the
        # source root - see https://fxbug.dev/478970857 so we must do so manually.
        # TODO(https://fxbug.dev/478896548): Make this `include_base` when updating to Bazel 9.
        import_include_path = native.package_name() + "/" + include_base

        cc_import(
            name = name,
            hdrs = hdrs_for_bazel_library,
            includes = [import_include_path],
            shared_library = ":%s" % exported_target_name,
            testonly = testonly,
            # In-tree code should depend on the imported shared library.
            visibility = visibility,
        )

        underlying_library_info_target_name = name + "_link_stubs"
        generate_companion_files_for_shared_library(
            name = underlying_library_info_target_name,
            shared_library = exported_target_name,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        )
    elif prebuilt_library_type == "static":
        # Create the exportable static library. This generates the `.a` file
        # that will be included in the IDK.
        exported_target_name = name + ".export/" + name

        cc_static_library(
            name = exported_target_name,
            deps = [":%s" % cc_library_name],
            testonly = testonly,
            # Only the IDK atom target should depend on this target.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        )

        underlying_library_info_target_name = name + "_static_library_info"
        get_library_info_for_static_library(
            name = underlying_library_info_target_name,
            static_library = exported_target_name,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        )
    else:
        fail("Unrecognized `prebuilt_library_type` '%s'." % prebuilt_library_type)

    #
    # Begin IDK atom creation.
    # Everything below this point is for Fuchsia only.
    #

    idk_root_path = "pkg/" + idk_name
    include_dest = idk_root_path + "/include"

    # Determine destinations in the IDK for headers and sources.
    idk_metadata_headers = []
    idk_header_files_map = {}

    # Note: Unlike `idk_cc_source_library()`, only relative `include_base`
    # values are allowed.
    for header in hdrs_for_idk:
        relative_destination = paths.relativize(header.name, include_base)
        destination = include_dest + "/" + relative_destination
        idk_metadata_headers.append(destination)
        idk_header_files_map |= {destination: header}

    # The binary files are added to `idk_files_map` by `_create_idk_atom()`.
    idk_files_map = dict(idk_header_files_map)

    atom_idk_deps = get_idk_deps(deps + fuchsia_deps + runtime_deps)

    # Dependencies for generating the actual IDK atom (not the underlying library).
    atom_build_deps = [
        # All this target really does is provide a clearer error message than if
        # we relied on combining the lists in the `verify_no_pragma_once()` rule
        # below.
        create_verify_no_duplicate_files_target(
            name = name,
            hdrs = hdrs,
            hdrs_for_internal_use = hdrs_for_internal_use,
            srcs = srcs,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        ),

        # For simplicity, check all source files, including non-header files in
        # `srcs`.
        create_verify_pragma_once_target(
            name = name,
            files = all_source_files,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        ),
    ]

    # IFS files do not apply to static libraries.
    must_verify_public_symbols = prebuilt_library_type != "static"

    if must_verify_public_symbols:
        verify_public_symbols_target_name = "%s.verify_public_symbols" % name

        # This target may be a no-op rule when targting "HEAD" because we do
        # not maintain golden IFS files for "HEAD".
        # TODO(https://fxbug.dev/417307356): Make the rule do nothing when
        # `ctx.attr._current_api_level[BuildSettingInfo].value == HEAD`.
        verify_public_symbols(
            name = verify_public_symbols_target_name,
            prebuilt_library = underlying_library_info_target_name,
            reference = ifs_golden_file,
            library_name = "//" + native.package_name() + ":" + name,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        )

        atom_build_deps.append(":%s" % verify_public_symbols_target_name)

    if stable and not no_headers:
        api_path = api_file_path
        api_contents_map = idk_header_files_map
    else:
        api_path = None
        api_contents_map = None

    # If changing this, also change
    # //build/sdk/idk_prebuild_manifest.gni:cc_prebuilt_library.
    additional_prebuild_info_values = {
        "format": prebuilt_library_type,
        "include_dir": include_dest,
        "headers": idk_metadata_headers,
        "file_base": idk_root_path,
        # "binaries" is added by `_create_idk_atom()`.
    }

    if no_headers and (api_path or api_contents_map or idk_header_files_map):
        fail("Internal error: Unexpected populated variables when `no_headers` is True.")

    atom_type = "cc_prebuilt_library"

    # Verify the allowlist here to catch cases where this macro is used but
    # there is no dependency on the atom target.
    verify_target_is_in_allowlist(
        name = name,
        type = atom_type,
        category = category,
        stable = stable,
        testonly = testonly,
        prebuilt_library_format = prebuilt_library_type,
    )

    idk_atom(
        name = name + "_idk",
        idk_name = idk_name,
        id = "sdk://" + idk_root_path,
        meta_dest = idk_root_path + "/meta.json",
        type = atom_type,
        category = category,
        stable = stable,
        api_area = api_area,
        api_file_path = api_path,
        api_contents_map = api_contents_map,
        files_map = idk_files_map,
        deps = atom_idk_deps,
        underlying_library = ":%s" % underlying_library_info_target_name,
        atom_build_deps = atom_build_deps,
        additional_prebuild_info = json_encode_dict_values(additional_prebuild_info_values),
        prebuilt_library_format = prebuilt_library_type,
        target_compatible_with = ["@platforms//os:fuchsia"],
        testonly = testonly,
        visibility = get_atom_visibility(visibility),
    )

_idk_cc_prebuilt_library = macro(
    doc = """Defines a C/C++ prebuilt library in the IDK.

Defines a prebuilt library of `prebuilt_library_type` named `name` and an IDK
atom named "{name}_idk". `name` must not begin with "lib".

The values of all deps args must be iterable. That means they cannot contain
`select()` statements. Instead, use `fuchsia_deps` for public dependencies
that only apply to Fuchsia.
""",
    implementation = _idk_cc_prebuilt_library_impl,
    # TODO(https://fxbug.dev/446694542): Remove `native.` once the
    # `cc_library()` wrapper is a symbolic macro.
    inherit_attrs = native.cc_library,
    attrs = {
        "prebuilt_library_type": attr.string(
            doc = """The type of the library - either "shared" or "static".""",
            mandatory = True,
            configurable = False,
        ),
        "idk_name": attr.string(
            doc = """Name of the library in the IDK. Usually matches `name`.
GN equivalent: `sdk_name`""",
            mandatory = True,
            configurable = False,
        ),
        "category": attr.string(
            doc = "Publication level of the library in the IDK. See _create_idk_atom().",
            values = ["partner"],
            mandatory = True,
            configurable = False,
        ),
        "stable": attr.bool(
            doc = """Whether this source library is stabilized. Set by the wrapper macro.
When True, a `.api` file is generated. When False, the atom is marked as unstable in the final IDK.""",
            mandatory = True,
            configurable = False,
        ),
        "api_area": attr.string(
            doc = """The API area responsible for maintaining this library.
GN equivalent: `sdk_area`""",
            mandatory = True,
        ),
        "hdrs": attr.label_list(
            doc = """The list of C and C++ header files published by this library to be directly
included by sources in dependent rules. Does not include internal headers that are included from
public headers but not meant to be included by dependents - see `hdrs_for_internal_use`.
Atoms providing headers used by these headers must be included in the (public) `deps`.
May only be empty if `no_headers` is True.
GN equivalent: `public`
GN note: Unlike the GN template, this list does not include `hdrs_for_internal_use`.""",
            allow_files = True,
            mandatory = True,
            configurable = False,
        ),
        "hdrs_for_internal_use": attr.label_list(
            doc = """List of C and C++ headers included by headers in `hdrs` that are not
meant to be included by a client of this library. They usually contain implementation details.
Their contents are not included in documentation but they are included in the `headers` metadata
for the IDK library. They may be excluded from some but not all API compatibility checks.
Like `hdrs`, the atoms providing headers used by these headers must be included in the
(public) `deps`.
Must be an empty list if `hdrs` is empty.
GN equivalent: `sdk_headers_for_internal_use`
GN note: Unlike the GN template where this argument specifices headers already included
elsewhere, such headers are only listed here.""",
            allow_files = True,
            default = [],
            configurable = False,
        ),
        "srcs": attr.label_list(
            doc = """The list of C and C++ source and header files that are processed to create the
library target, excluding those in `hdrs` and `hdrs_for_internal_use`.
Header files in this list can only be included by other files in this list.
GN equivalent: `sources`
GN note: Unlike the GN template, public headers must actually be in `hdrs`.""",
            allow_files = True,
            mandatory = True,
            configurable = False,
        ),
        "deps": attr.label_list(
            doc = """List of labels for other IDK elements this element publicly depends on at build time.
These labels must point to targets with corresponding `_create_idk_atom()` targets.
As with all deps arguments, must not contain `select()` statements.
GN equivalent: `public_deps`""",
            default = [],
            configurable = False,
        ),
        "fuchsia_deps": attr.label_list(
            doc = """List of labels for other IDK elements this element publicly depends on at
build time only when targeting Fuchsia.
These labels must point to targets with corresponding `_create_idk_atom()` targets.
GN equivalent: `public_deps +=` inside an `if (is_fuchsia) {}` statement.
GN note: If `bazel2gn` is run on the target, `fuchsia_deps` must come after `deps.
This may require adding `# buildifier: leave-alone` above the target definition to
disable reordering by the formatter.""",
            default = [],
            configurable = False,
        ),
        "implementation_deps": attr.label_list(
            doc = """List of labels this element depends on at build time.
GN equivalent: `deps`.""",
            default = [],
        ),
        "runtime_deps": attr.label_list(
            doc = """List of labels representing the library's runtime dependencies.
This is only only needed for runtime dependencies inherited from private
dependencies (`implementation_deps` or indirect dependencies not through an IDK
atom's public `deps`). Only labels for shared objects with a corresponding IDK
atom should be specified. That is, labels of instances of
`idk_cc_shared_library()` without the `_idk` suffix.
When not empty, `implementation_deps` must not be empty.
TODO(https://fxbug.dev/447151364): Migrate the note on runtime dependencies from GN template and reference it here.
GN equivalent: `runtime_deps`
GN note: Unlike the GN template, dependencies are not the atom targets and thus
do not end in `_idk`.""",
            default = [],
            configurable = False,
        ),
        "include_base": attr.string(
            doc = """Path to the root directory for includes.
This path will be added to the underlying library's `includes`.
Must be relative to the directory containing the invoking BUILD.bazel file.
`include_base` will be removed from all header paths.""",
            default = "include",
            configurable = False,
        ),
        "api_file_path": attr.label(
            doc = """Override path for the file representing the API of this library.
This file is used to ensure modifications to the library's API are explicitly acknowledged.
Not allowed when `no_headers` is True.
When using the wrapper macro:
  * If not specified, the path will be "<idk_name>.api".
  * Only specify when the default needs to be overridden.
When the path is not in the current directory, the file will likely need to be
made visibile to this target using `exports_files()` in the BUILD.bazel file
for the directory containing the .api file.
GN equivalent: `api`""",
            allow_single_file = True,
            configurable = False,
        ),
        "output_name": attr.string(
            doc = """Name of the library to generate. Defaults to `name`.
Will be appended to "lib" to generate the library file name. Must not begin with "lib".
Required for shared libraries. Not supported for static libraries.""",
            configurable = False,
        ),
        "no_headers": attr.bool(
            doc = """Specifies that the library's headers are NOT included in the IDK.
Must be False unless `prebuilt_library_type` is "shared".
When True, the API modification acknowledgement mechanism is disabled. (Only the
IFS file mechanism will be used.)
When true, `hdrs` and `deps` must be an empty lists.""",
            default = False,
            configurable = False,
        ),
        "libcxx_linkage": attr.string(
            doc = """Whether or how to link libc++.
SDK shared libraries cannot link libc++.so dynamically because libc++.so does
not have a stable ABI. Can be either "none" or "static".""",
            default = "none",
            configurable = False,
        ),
        "ifs_golden_file": attr.label(
            doc = "The golden IFS file for shared libraries only. Set by the wrapper macro.",
            mandatory = False,
        ),
        # TODO(https://fxbug.dev/425931839): Remove these when no longer converting to GN.
        "friend": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "public_configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "version_script": attr.string(
            doc = "The version script for the shared library.",
            default = "",
            configurable = False,
        ),
        "ldflags": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "inputs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),

        # Require use of `runtime_deps` for atoms that are runtime dependencies. Do not inherit.
        "data": None,
        # Do not inherit as this attribute is specified to `cc_library()` in the implementation.
        "includes": None,
    },
)

# LINT.ThenChange(//build/cpp/sdk_prebuilt_library_impl.gni:sdk_prebuilt_library_impl)

def _idk_cc_shared_library_impl(name, **kwargs):
    _idk_cc_prebuilt_library(name = name, prebuilt_library_type = "shared", **kwargs)

idk_cc_shared_library = macro(
    doc = """Defines a C/C++ prebuilt shared library in the IDK.

Use the `idk_cc_shared_library()` wrapper instead.
""",
    inherit_attrs = _idk_cc_prebuilt_library,
    attrs = {
        # Do not inherit as this attribute is specified in the implementation.
        "prebuilt_library_type": None,
        # These attributes are mandatory for shared libraries.
        "output_name": attr.string(
            doc = """Name of the library to generate. Defaults to `name`.
Will be appended to "lib" to generate the library file name. Must not begin with "lib".""",
            mandatory = True,
            configurable = False,
        ),
        "ifs_golden_file": attr.label(
            doc = "The golden IFS file for shared libraries. Set by the wrapper macro.",
            mandatory = True,
        ),
    },
    implementation = _idk_cc_shared_library_impl,
)

def _idk_cc_static_library_impl(name, **kwargs):
    _idk_cc_prebuilt_library(name = name, prebuilt_library_type = "static", **kwargs)

idk_cc_static_library = macro(
    doc = """Defines a C/C++ prebuilt static library in the IDK.

Use the `idk_cc_static_library()` wrapper instead.
""",
    inherit_attrs = _idk_cc_prebuilt_library,
    attrs = {
        # Do not inherit as this attribute is specified in the implementation.
        "prebuilt_library_type": None,
        # Disallow an empty list, unlike the underlying macro.
        "hdrs": attr.label_list(
            doc = """The list of C and C++ header files published by this library to be directly
included by sources in dependent rules. Does not include internal headers that are included from
public headers but not meant to be included by dependents - see `hdrs_for_internal_use`.
Atoms providing headers used by these headers must be included in the (public) `deps`.
May only be empty if `no_headers` is True.
GN equivalent: `public`
GN note: Unlike the GN template, this list does not include `hdrs_for_internal_use`.""",
            allow_files = True,
            allow_empty = False,
            mandatory = True,
            configurable = False,
        ),
        # Do not inherit as this attribute is not supported.
        "output_name": None,
    },
    implementation = _idk_cc_static_library_impl,
)

def _idk_cc_shared_library_zx_impl(
        name,
        **kwargs):
    """Implementation for the _idk_cc_shared_library_zx() macro."""

    kwargs = apply_common_zx_library_modifications(kwargs)

    idk_cc_shared_library(
        name = name,
        **kwargs
    )

idk_cc_shared_library_zx = macro(
    doc = """Defines a C/C++ prebuilt shared library in the IDK that will be a `zx_library()` in GN.

Use the `idk_cc_shared_library_zx()` wrapper instead.

When not using a Zircon-specific toolchain:
 * Any ":headers" or ":<library>.headers" targets that appear in public
   dependencies will be rewritten into a dependency on the library itself.
   For example:
        deps = [ "//zircon/system/ulib/foo:headers", "//zircon/system/ulib/bar:bar.headers" ]
    will be replaced by:
        deps = [ "//zircon/system/ulib/foo", "//zircon/system/ulib/bar" ]

 * Any ":<library>.as-needed" targets that appear in private dependencies
   will be rewritten into a dependency on the library itself.
   For example:
        implementation_deps = [ "//zircon/system/ulib/bar:bar.as-needed" ]
    will be replaced by:
        implementation_deps = [ "//zircon/system/ulib/bar" ]
""",
    inherit_attrs = idk_cc_shared_library,
    implementation = _idk_cc_shared_library_zx_impl,
    attrs = {
        # Override these attrs to document the differences from the GN `zx_library()` template.
        "category": attr.string(
            doc = """See idk_cc_shared_library().
GN equivalent: `sdk_publishable`""",
            values = ["partner"],
            mandatory = True,
            configurable = False,
        ),
        "hdrs": attr.label_list(
            doc = """See idk_cc_shared_library().
GN equivalent: `sdk_headers`
GN note: Unlike the GN template, the "include/" part of the path must be specified.""",
            allow_files = True,
            mandatory = True,
            configurable = False,
        ),
        # Override the inherited version to make it non-configurable.
        "implementation_deps": attr.label_list(
            doc = """List of labels this element depends on at build time.
GN equivalent: `deps`.""",
            default = [],
            configurable = False,
        ),
        # zx libraries always use "include" (the default) as the include base. Do not inherit.
        "include_base": None,
    },
)

def _idk_cc_static_library_zx_impl(
        name,
        **kwargs):
    """Implementation for the _idk_cc_static_library_zx() macro."""

    kwargs = apply_common_zx_library_modifications(kwargs)

    idk_cc_static_library(
        name = name,
        **kwargs
    )

idk_cc_static_library_zx = macro(
    doc = """Defines a C/C++ prebuilt static library in the IDK that will be a `zx_library()` in GN.

Use the `idk_cc_static_library_zx()` wrapper instead.

When not using a Zircon-specific toolchain:
 * Any ":headers" or ":<library>.headers" targets that appear in public
   dependencies will be rewritten into a dependency on the library itself.
   For example:
        deps = [ "//zircon/system/ulib/foo:headers", "//zircon/system/ulib/bar:bar.headers" ]
    will be replaced by:
        deps = [ "//zircon/system/ulib/foo", "//zircon/system/ulib/bar" ]

 * Any ":<library>.as-needed" targets that appear in private dependencies
   will be rewritten into a dependency on the library itself.
   For example:
        implementation_deps = [ "//zircon/system/ulib/bar:bar.as-needed" ]
    will be replaced by:
        implementation_deps = [ "//zircon/system/ulib/bar" ]
""",
    inherit_attrs = idk_cc_static_library,
    implementation = _idk_cc_static_library_zx_impl,
    attrs = {
        # Override these attrs to document the differences from the GN `zx_library()` template.
        "category": attr.string(
            doc = """See idk_cc_static_library().
GN equivalent: `sdk_publishable`""",
            values = ["partner"],
            mandatory = True,
            configurable = False,
        ),
        "hdrs": attr.label_list(
            doc = """See idk_cc_static_library().
GN equivalent: `sdk_headers`
GN note: Unlike the GN template, the "include/" part of the path must be specified.""",
            allow_files = True,
            mandatory = True,
            configurable = False,
        ),
        # Override the inherited version to make it non-configurable.
        "implementation_deps": attr.label_list(
            doc = """List of labels this element depends on at build time.
GN equivalent: `deps`.""",
            default = [],
            configurable = False,
        ),
        # zx libraries always use "include" (the default) as the include base. Do not inherit.
        "include_base": None,
    },
)
