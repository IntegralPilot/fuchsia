# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for defining IDK host tools."""

load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
load("//build/bazel/rules/host:defs.bzl", "cc_binary_host_tool", "go_binary_host_tool", "rustc_binary_host_tool")
load(":idk_atom.bzl", "idk_atom")
load(
    ":idk_common.bzl",
    "get_atom_visibility",
    "json_encode_dict_values",
    "verify_target_is_in_allowlist",
)

visibility([
    "//build/bazel/rules/idk/...",
])

def _idk_host_tool_atom_impl(
        name,
        idk_name,
        category,
        api_area,
        tool,
        output_name,
        target_compatible_with,
        visibility):
    if not name.endswith("_idk"):
        fail('IDK atom `name`s must end with "_idk".')
    if target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_OS_CONSTRAINTS`.")

    if not output_name:
        output_name = idk_name

    file_base = "tools"
    idk_path = file_base + "/$<current_cpu>/%s" % idk_name

    additional_prebuild_info_values = {
        "file_base": file_base,
    }

    atom_type = "host_tool"

    # Verify the allowlist here to catch cases where this macro is used but
    # there is no dependency on the atom target.
    verify_target_is_in_allowlist(
        name = name.removesuffix("_idk"),
        type = atom_type,
        category = category,
        stable = True,
        testonly = False,
    )

    idk_atom(
        name = name,
        idk_name = idk_name,
        id = "sdk://" + idk_path,
        meta_dest = idk_path + "-meta.json",
        type = atom_type,
        category = category,
        stable = True,
        api_area = api_area,
        files_map = {idk_path: tool},
        atom_build_deps = [],
        additional_prebuild_info = json_encode_dict_values(additional_prebuild_info_values),
        visibility = get_atom_visibility(visibility),
        target_compatible_with = target_compatible_with,
    )

_idk_host_tool_atom = macro(
    doc = """Defines a host tool in the IDK.

    `name` must end with "_idk".
    """,
    implementation = _idk_host_tool_atom_impl,
    attrs = {
        "idk_name": attr.string(
            doc = """Name of the tool in the IDK. Usually matches `name`.
GN equivalent: `sdk_name`""",
            mandatory = True,
            configurable = False,
        ),
        "category": attr.string(
            doc = "Publication level of the tool in the IDK. See _create_idk_atom().",
            values = ["partner"],
            mandatory = True,
            configurable = False,
        ),
        "api_area": attr.string(
            doc = """The API area responsible for maintaining this tool.
GN equivalent: `sdk_area`""",
            mandatory = True,
        ),
        "tool": attr.label(
            doc = """Label of the tool to be added to the IDK.
GN equivalent: `deps`.""",
            mandatory = True,
            configurable = False,
        ),
        # TODO(https://fxbug.dev/425931839): Remove if unused after all tools are migrated.
        "output_name": attr.string(
            doc = """The tool's name. Defaults to `idk_name`.
GN note: The default relationship to `idk_name` is different from GN.""",
        ),
        # TODO(https://fxbug.dev/442025401): Consider implementing this within
        # bazel2gn rather than requiring it at each call site.
        # TODO(https://fxbug.dev/460538634): Replace with the following once
        # bazel2gn is no longer being used for host tools.
        # "target_compatible_with": None,
        "target_compatible_with": attr.string_list(
            doc = "Standard meaning. Must be `HOST_OS_CONSTRAINTS`.",
            mandatory = False,
            configurable = False,
            default = HOST_OS_CONSTRAINTS,
        ),
    },
)

_BINARY_HOST_TOOL_ATTRS = {
    "idk_name": attr.string(
        doc = "The name of the tool in the IDK. Usually matches `name`.",
        mandatory = True,
        configurable = False,
    ),
    "category": attr.string(
        doc = "Publication level of the tool in the IDK. See _create_idk_atom().",
        mandatory = True,
        configurable = False,
    ),
    "api_area": attr.string(
        doc = "The API area responsible for maintaining this tool.",
        mandatory = True,
    ),
    # TODO(https://fxbug.dev/442025401): Consider implementing this within
    # bazel2gn rather than requiring it at each call site.
    # TODO(https://fxbug.dev/460538634): Replace with the following once
    # bazel2gn is no longer being used for host tools.
    # "target_compatible_with": None,
    "target_compatible_with": attr.string_list(
        doc = "Standard meaning. Must be `HOST_OS_CONSTRAINTS`.",
        default = HOST_OS_CONSTRAINTS,
        configurable = False,
    ),
}

def _idk_cc_binary_host_tool_impl(
        name,
        idk_name,
        category,
        api_area,
        target_compatible_with,
        visibility,
        **kwargs):
    if "idk" in name or "sdk" in name:
        fail('`name`s must not include "idk" or "sdk".')
    if target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_OS_CONSTRAINTS`.")

    binary_name = name

    cc_binary_host_tool(
        name = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
        visibility = visibility,
        **kwargs
    )

    _idk_host_tool_atom(
        name = name + "_idk",
        idk_name = idk_name,
        category = category,
        api_area = api_area,
        tool = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
        visibility = visibility,
    )

idk_cc_binary_host_tool = macro(
    doc = """Defines a `cc_binary()` host tool in the IDK.

    GN note: Unlike some GN templates, `name` should not include "_sdk"/"_idk".
    """,
    implementation = _idk_cc_binary_host_tool_impl,
    inherit_attrs = cc_binary_host_tool,
    attrs = _BINARY_HOST_TOOL_ATTRS,
)

# This must be a legacy macro with `**kwargs` because go_binary_host_tool is a
# legacy macro, which cannot be used with `inherit_attrs` in a symbolic macro.
def idk_go_binary_host_tool(
        *,
        name,
        idk_name,
        category,
        api_area,
        visibility = None,
        # TODO(https://fxbug.dev/460538634): Remove once bazel2gn is no longer
        # being used for host tools.
        target_compatible_with = HOST_OS_CONSTRAINTS,
        **kwargs):
    """Defines a host tool in the IDK for a `go_binary()` tool.

    Args:
        name: The name of the tool binary.
        idk_name: The name of the tool in the IDK. Usually matches `name`.
        category: Publication level of the tool in the IDK. See _create_idk_atom().
        api_area: The API area responsible for maintaining this tool.
        visibility: Standard meaning.
        # TODO(https://fxbug.dev/442025401): Consider implementing this within
        # bazel2gn rather than requiring it at each call site.
        # TODO(https://fxbug.dev/460538634): Remove once bazel2gn is no longer
        # being used for host tools.
        target_compatible_with: Standard meaning. Must be `HOST_OS_CONSTRAINTS`.
        **kwargs: Passed to `go_binary()`.

    GN note: Unlike some GN templates, `name` should not include "_sdk"/"_idk".
    """
    if "idk" in name or "sdk" in name:
        fail('`name`s must not include "idk" or "sdk".')
    if target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_OS_CONSTRAINTS`.")

    binary_name = name

    go_binary_host_tool(
        name = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
        visibility = visibility,
        **kwargs
    )

    _idk_host_tool_atom(
        name = name + "_idk",
        idk_name = idk_name,
        category = category,
        api_area = api_area,
        tool = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
        visibility = visibility,
    )

def _idk_rustc_binary_host_tool_impl(
        name,
        idk_name,
        category,
        api_area,
        target_compatible_with,
        **kwargs):
    if target_compatible_with != HOST_OS_CONSTRAINTS:
        fail("`target_compatible_with` must be `HOST_OS_CONSTRAINTS`.")

    binary_name = name

    rustc_binary_host_tool(
        name = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
        **kwargs
    )

    _idk_host_tool_atom(
        name = name + "_idk",
        idk_name = idk_name,
        category = category,
        api_area = api_area,
        tool = binary_name,
        target_compatible_with = HOST_OS_CONSTRAINTS,
    )

idk_rustc_binary_host_tool = macro(
    doc = """Defines a `rustc_binary()` host tool in the IDK.

    GN note: Unlike some GN templates, `name` should not include "_sdk"/"_idk".
    """,
    implementation = _idk_rustc_binary_host_tool_impl,
    inherit_attrs = rustc_binary_host_tool,
    attrs = _BINARY_HOST_TOOL_ATTRS,
)
