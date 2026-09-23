# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines an IDK C/C++ source library."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@rules_cc//cc:defs.bzl", "cc_library")
load("//build/bazel/rules:zx_library.bzl", "apply_common_zx_library_modifications")
load(
    ":cc_verification.bzl",
    "create_verify_no_duplicate_files_target",
    "create_verify_pragma_once_target",
)
load(":idk_atom.bzl", "ConfigurableInfo", "idk_atom")
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

# LINT.IfChange(idk_cc_source_library)

def _get_include_path_for_cc_library(include_base):
    """Return the include path to use for the underlying library for the given `include_base`.

    Some libraries in //sdk/lib/<library_name>[/...] rely on that in-tree
    path to allow in-tree code to include `<lib/library_name/header.h>`
    and for the IDK destination path rather than providing that structure
    within the `library_name` directory.

    In this case, return the relative path from the build file's directory to
    `//sdk`. This is necessary because `cc_library()` does not support absolute
    include paths.

    Otherwise, return `include_base`.
    """
    if include_base != "//sdk":
        return include_base

    # Get the relative path from the build file's directory to `//sdk`,
    # which is the real include base for in-tree builds.
    path_to_this_directory = "//" + native.package_name()
    this_directory_relative_to_sdk = paths.relativize(path_to_this_directory, "//sdk")
    include_path = ""
    for _ in this_directory_relative_to_sdk.split("/"):
        include_path += "../"
    return include_path

def _build_configurable_info_impl(ctx):
    this_directory_relative_to_sdk = None  # Silence buildifier warning.
    if ctx.attr.include_base == "//sdk":
        path_to_this_directory = "//" + ctx.label.package
        this_directory_relative_to_sdk = paths.relativize(path_to_this_directory, "//sdk")

    idk_metadata_headers = []
    idk_header_files_map = {}
    idk_header_files = []

    for header, header_file in zip(ctx.attr.hdrs, ctx.files.hdrs):
        if ctx.attr.include_base == "//sdk":
            # Handle the special case.
            relative_destination = paths.join(this_directory_relative_to_sdk, header.label.name)

        else:
            relative_destination = paths.relativize(header.label.name, ctx.attr.include_base)
        destination = ctx.attr.include_dest + "/" + relative_destination
        idk_metadata_headers.append(destination)
        idk_header_files_map[destination] = header
        idk_header_files.append(header_file)

    files_map = dict(idk_header_files_map)

    idk_metadata_sources = []
    for source in ctx.attr.srcs:
        source_dest_path = ctx.attr.idk_root_path + "/" + source.label.name
        idk_metadata_sources.append(source_dest_path)
        files_map[source_dest_path] = source

    return [
        DefaultInfo(files = depset(ctx.files.hdrs + ctx.files.srcs)),
        ConfigurableInfo(
            api_contents_map = idk_header_files_map if ctx.attr.stable else {},
            api_contents_map_files = idk_header_files if ctx.attr.stable else [],
            files_map = files_map,
            additional_prebuild_info_values = {
                "sources": json.encode(idk_metadata_sources),
                "headers": json.encode(idk_metadata_headers),
            },
        ),
    ]

build_configurable_info = rule(
    doc = "Adds potentially-configurable properties to a `ConfigurableInfo`.",
    implementation = _build_configurable_info_impl,
    attrs = {
        "idk_root_path": attr.string(mandatory = True),
        "stable": attr.bool(mandatory = True),
        "include_dest": attr.string(mandatory = True),
        "include_base": attr.string(default = "include"),
        "hdrs": attr.label_list(allow_files = True),
        "srcs": attr.label_list(allow_files = True),
    },
)

# TODO(https://fxbug.dev/428229472): When migrating "zbi-format":
# * add `non_idk_implementation_deps` (GN equivalent: `non_sdk_deps`) argument.
# * assert that it is only used for "//sdk/fidl/zbi:zbi.c.checked-in".
# * Add TODO for https://fxbug.dev/42062786 to remove the argument when fixing the bug.
def _idk_cc_source_library_impl(
        name,
        idk_name,
        category,
        stable,
        api_area,
        hdrs,
        hdrs_for_internal_use,
        srcs,
        deps,
        fuchsia_deps,
        non_fuchsia_deps,
        implementation_deps,
        fuchsia_implementation_deps,
        include_base,
        api_file_path,
        testonly,
        visibility,
        build_as_static,  # buildifier: disable=unused-variable - For GN conversion only.
        friend,  # buildifier: disable=unused-variable - For GN conversion only.
        public_configs,  # buildifier: disable=unused-variable - For GN conversion only.
        configs,  # buildifier: disable=unused-variable - For GN conversion only.
        **kwargs):
    """Implementation for the idk_cc_source_library() macro."""
    if "data" in kwargs:
        fail("Rumtime dependencies are not supported for source libraries.")

    if category not in ["partner"]:
        # Other categories are only to ensure ABI compatibility and thus not
        # applicable.
        fail("Category '%s' is not supported." % category)

    if api_file_path and not stable:
        fail("Unstable targets do not require/support modification acknowledgement.")

    # Group the source files for various uses.
    # Per https://bazel.build/reference/be/c-cpp#cc_library.hdrs,
    # "Headers not meant to be included by a client of this library should be
    # listed in the srcs attribute instead, even if they are included by a
    # published header." Thus, `hdrs_for_internal_use` is added to `srcs` in the
    # underlying library, not `hdrs`. However, other build systems that use the
    # IDK may not work this way, so include `hdrs_for_internal_use` as headers
    # in the IDK. This is also consistent with prebuilt libraries where the IDK
    # only includes headers.
    hdrs_for_idk = hdrs + hdrs_for_internal_use
    srcs_for_idk = srcs
    hdrs_for_bazel_library = hdrs
    srcs_for_bazel_library = srcs + hdrs_for_internal_use

    # TODO(https://fxbug.dev/421888626): Apply the equivalent of GN's
    # `default_common_binary_configs` using the `copts` attribute.
    # TODO(https://fxbug.dev/421888626): Add "//build/config:sdk_extra_warnings"
    # using the `copts` attribute.
    cc_library(
        name = name,
        srcs = srcs_for_bazel_library,
        hdrs = hdrs_for_bazel_library,
        deps = deps + select_for_fuchsia(
            fuchsia_deps,
            non_fuchsia_value = non_fuchsia_deps,
        ),
        # TODO(https://fxbug.dev/428229472): If we must support
        # `non_idk_implementation_deps`, include it below.
        implementation_deps = implementation_deps + select_for_fuchsia(fuchsia_implementation_deps),
        includes = [_get_include_path_for_cc_library(include_base)],
        testonly = testonly,
        # Allow access from //sdk:all_underlying_source_libraries.
        visibility = visibility + ["//sdk:__pkg__"],
        **kwargs
    )

    #
    # Begin IDK atom creation.
    # Everything below this point is for Fuchsia only.
    #

    all_idk_source_files = hdrs_for_idk + srcs_for_idk

    idk_root_path = "pkg/" + idk_name
    include_dest = idk_root_path + "/include"

    atom_idk_deps = get_idk_deps(deps + fuchsia_deps + implementation_deps + fuchsia_implementation_deps)

    # Dependencies for generating the actual IDK atom (not the underlying library).
    # TODO(https://fxbug.dev/428229472): If we must support
    # `non_idk_implementation_deps`, add it here.
    atom_build_deps = [
        # All this target really does is provide a clearer error message than if
        # we relied on combining the lists in the `verify_no_pragma_once()` rule
        # below. Only files in the IDK (platform-independent and
        # Fuchsia-specific) are checked.
        create_verify_no_duplicate_files_target(
            name = name,
            hdrs = hdrs,
            hdrs_for_internal_use = hdrs_for_internal_use,
            srcs = srcs_for_idk,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        ),

        # For simplicity, check all source files, including non-header files in
        # `*srcs`. Only files in the IDK (platform-independent and
        # Fuchsia-specific) are checked.
        create_verify_pragma_once_target(
            name = name,
            files = all_idk_source_files,
            testonly = testonly,
            # Required for tests using `create_test_atom_info()`.
            visibility = ["//build/bazel/bazel_idk/tests:__subpackages__"],
        ),
    ]

    api_path = api_file_path if stable else None

    # If changing this, also change
    # //build/sdk/idk_prebuild_manifest.gni:cc_source_library.
    additional_prebuild_info_values = {
        "include_dir": include_dest,
        "file_base": idk_root_path,
        # "sources" will be added via `configurable_info_name`.
        # "headers" will be added via `configurable_info_name`.
    }

    # Some of the attributes we need to generate IDK metadata are configurable,
    # which means they can only be processed in rules. This rule processes
    # the relevant attributes and puts the results in a `ConfigurableInfo`
    # provider that can be passed to the atom rule for use.
    configurable_info_name = name + "_configurable_info"
    build_configurable_info(
        name = configurable_info_name,
        idk_root_path = idk_root_path,
        stable = stable,
        include_dest = include_dest,
        include_base = include_base,
        hdrs = hdrs_for_idk,
        srcs = srcs_for_idk,
    )

    atom_type = "cc_source_library"

    # Verify the allowlist here to catch cases where this macro is used but
    # there is no dependency on the atom target.
    verify_target_is_in_allowlist(
        name = name,
        type = atom_type,
        category = category,
        stable = stable,
        testonly = testonly,
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
        deps = atom_idk_deps,
        atom_build_deps = atom_build_deps,
        additional_prebuild_info = json_encode_dict_values(additional_prebuild_info_values),
        configurable_info = ":" + configurable_info_name,
        target_compatible_with = ["@platforms//os:fuchsia"],
        testonly = testonly,
        visibility = get_atom_visibility(visibility),
    )

idk_cc_source_library = macro(
    doc = """Defines a C/C++ source library in the IDK.

Use the `idk_cc_source_library()` wrapper instead.

Defines a `cc_library` named `name` and an IDK atom named "{name}_idk".

The IDK atom target ("{name}_idk") does NOT build the underlying library
(`name`). To provide build coverage, ensure some other target in the build
graph depends on target `name`.

The values of all deps args must be iterable. That means they cannot contain
`select()` statements. Instead, use `fuchsia_deps` for public dependencies
that only apply to Fuchsia.""",
    implementation = _idk_cc_source_library_impl,
    # TODO(https://fxbug.dev/446694542): Remove `native.` once the
    # `cc_library()` wrapper is a symbolic macro.
    inherit_attrs = native.cc_library,
    attrs = {
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
            doc = """Whether this source library is stabilized.
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
GN equivalent: `public`
GN note: Unlike the GN template, this list does not include `hdrs_for_internal_use`.""",
            allow_files = True,
            allow_empty = False,
            mandatory = True,
        ),
        "hdrs_for_internal_use": attr.label_list(
            doc = """List of C and C++ headers included by headers in `hdrs` that are not
meant to be included by a client of this library. They usually contain implementation details.
Their contents are not included in documentation but they are included in the `headers` metadata
for the IDK library. They may be excluded from some but not all API compatibility checks.
Like `hdrs`, the atoms providing headers used by these headers must be included in the
(public) `deps`.
GN equivalent: `sdk_headers_for_internal_use`
GN note: Unlike the GN template where this argument specifices headers already included
elsewhere, such headers are only listed here.""",
            allow_files = True,
            default = [],
        ),
        "srcs": attr.label_list(
            doc = """The list of C and C++ source and header files that are processed to create the
library target, excluding those in `hdrs` and `hdrs_for_internal_use`.
Header files in this list can only be included by other files in `*srcs` attributes.
GN equivalent: `sources`
GN note: Unlike the GN template, public headers must actually be in `hdrs`.""",
            allow_files = True,
            default = [],
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
GN equivalent: `public_deps +=` inside an `if (is_fuchsia) {}` statement
GN note: If `bazel2gn` is run on the target, `fuchsia_deps` must come after `deps`.
This may require adding `# buildifier: leave-alone` above the target definition to
disable reordering by the formatter.""",
            default = [],
            configurable = False,
        ),
        "non_fuchsia_deps": attr.label_list(
            doc = """List of labels for other IDK elements this element publicly depends on at
build time only when not targeting Fuchsia.
These labels do not need to point to targets with corresponding `_create_idk_atom()` targets.
GN equivalent: `public_deps +=` inside an `if (!is_fuchsia) {}` statement""",
            default = [],
            configurable = False,
        ),
        "implementation_deps": attr.label_list(
            doc = """List of labels for other IDK elements this element depends on at build time.
These labels must point to targets with corresponding `_create_idk_atom()` targets.
GN equivalent: `deps`.""",
            default = [],
            configurable = False,
        ),
        "fuchsia_implementation_deps": attr.label_list(
            doc = """List of labels for other IDK elements this element depends
on at build time only when targeting Fuchsia.
These labels must point to targets with corresponding `_create_idk_atom()` targets.
GN equivalent: `deps +=` inside an `if (is_fuchsia) {}` statement
GN note: If `bazel2gn` is run on the target, `fuchsia_implementation_deps` must
come after `implementation_deps`. This may require adding
`# buildifier: leave-alone` above the target definition to disable reordering by
the formatter.""",
            default = [],
            configurable = False,
        ),
        "include_base": attr.string(
            doc = """Path to the root directory for includes.
This path will be added to the underlying library's `includes`.
If the path is "//sdk", the paths to the headers will be made relative to `//sdk`.
therwise, it must be relative to the directory containing the invoking
BUILD.bazel file and `include_base` will be removed from all header paths.
GN note: This preserves the behavior of the GN template.""",
            default = "include",
            configurable = False,
        ),
        "api_file_path": attr.label(
            doc = """Override path for the file representing the API of this library.
This file is used to ensure modifications to the library's API are explicitly acknowledged.
Must be specified to this macro if and only if `stable` is true.
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
        # TODO(https://fxbug.dev/425931839): Remove these when no longer converting to GN.
        # TODO(https://fxbug.dev/421888626): Use this argument if there is a
        # way to tell Bazel to not always compile the source set.
        "build_as_static": attr.bool(
            doc = "Unused in Bazel, for GN conversion only.",
            default = False,
        ),
        "friend": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "public_configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        # Rumtime dependencies are not supported for source libraries. Do not inherit.
        "data": None,
        # Do not inherit as this attribute is specified to `cc_library()` in the implementation.
        "includes": None,
    },
)

# LINT.ThenChange(//build/cpp/sdk_source_set.gni)

def _idk_cc_source_library_zx_impl(
        name,
        **kwargs):
    """Implementation for the idk_cc_source_library_zx() macro."""

    kwargs = apply_common_zx_library_modifications(kwargs)

    idk_cc_source_library(
        name = name,
        **kwargs
    )

idk_cc_source_library_zx = macro(
    doc = """Defines a C/C++ source library in the IDK and will be a `zx_library()` in GN.

Use the `idk_cc_source_library_zx()` wrapper instead.

Bazel may create a static library as it does not have a concept of source libraries.

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
    inherit_attrs = idk_cc_source_library,
    implementation = _idk_cc_source_library_zx_impl,
    attrs = {
        # Override these attrs to document the differences from the GN `zx_library()` template.
        "category": attr.string(
            doc = """See idk_cc_source_library().
GN equivalent: `sdk_publishable`""",
            values = ["partner"],
            mandatory = True,
            configurable = False,
        ),
        "hdrs": attr.label_list(
            doc = """See idk_cc_source_library().
GN equivalent: `sdk_headers`
GN note: Unlike the GN template, the "include/" part of the path must be specified.""",
            allow_files = True,
            mandatory = True,
        ),
        # zx libraries always use "include" (the default) as the include base. Do not inherit.
        "include_base": None,
    },
)
