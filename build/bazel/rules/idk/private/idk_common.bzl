# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common functions for IDK macros."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@fuchsia_build_info//:args.bzl", "idk_buildable_api_levels")
load(
    "//sdk:atom_lists.bzl",
    "ALL_HOST_TOOL_ATOMS",
    "CC_PREBUILT_SHARED_LIBRARY_ATOMS",
    "CC_PREBUILT_STATIC_LIBRARY_ATOMS",
    "DATA_ATOMS",
    "NOOP_ATOMS_LIST",
    "STABLE_CC_SOURCE_LIBRARY_ATOMS",
    "UNSTABLE_CC_SOURCE_LIBRARY_ATOMS",
)
load(
    "//sdk/fidl:category_lists.bzl",
    "COMPAT_TEST_FIDL_LIBRARY_ATOMS_LIST",
    "HOST_TOOL_FIDL_LIBRARY_ATOMS_LIST",
    "PARTNER_IDK_STABLE_FIDL_LIBRARY_ATOMS_LIST",
    "PARTNER_IDK_UNSTABLE_FIDL_LIBRARY_ATOMS_LIST",
    "PREBUILT_FIDL_LIBRARY_ATOMS_LIST",
)

visibility([
    "//build/bazel/rules/fidl/...",
    "//build/bazel/rules/idk/...",
    "//build/sdk/...",
])

def json_encode_dict_values(dict):
    """Returns the dictionary with each top-level value encoded as a JSON string.

This allows the dictionary to be passed to a `string_dict` attribute.
    """
    return {k: json.encode(v) for k, v in dict.items()}

def select_for(condition, condition_value, *, default_value = []):
    return select({
        condition: condition_value,
        "//conditions:default": default_value,
    })

def select_for_fuchsia(fuchsia_value, *, non_fuchsia_value = []):
    return select_for(
        "@platforms//os:fuchsia",
        fuchsia_value,
        default_value = non_fuchsia_value,
    )

def _get_idk_label(label_str):
    # Ensure the label is relative to the `BUILD` file, not this `.bzl` file
    # in cases where `label_str` omits the package (e.g., ":target_name").
    label = native.package_relative_label(label_str)

    # Build the label to handle cases where `label_str` omits the target name
    # (e.g., "//path/to/package").
    return "//{}:{}_idk".format(label.package, label.name)

# Buildifier considers this to be a macro because `_get_idk_label()` calls
# `native.package_relative_label()`, which it considers to be a rule. However,
# both are functions, not macros. See https://fxbug.dev/524667764.
# buildifier: disable=unnamed-macro
def get_idk_deps(underlying_deps):
    return [_get_idk_label(dep) for dep in underlying_deps]

def verify_target_is_in_allowlist(
        *,
        name,
        type,
        category,
        stable,
        testonly,
        prebuilt_library_format = None):
    """Verifies that the atom for non-IDK target `name` is in the allowlist for its type, category, and stability.

    If the atom fails verification, `fail()` is called with a message describing
    the issue. Otherwise, the function returns without side effects. No target
    is created.

    All atom-defining macros must call this when a category is specified if they
    expose any targets (e.g., a `cc_library()`) other than the IDK atom.

    All atoms must be in an allowlist.

    Args:
        name: The name of the atom to verify. It is a name (not label) of a
            target in the current package and does not have the "_idk" suffix.
        type: The atom's type.
        category: The atom's category.
        stable: Whether the atom is stable.
        testonly: Standard meaning.
        prebuilt_library_format: The format of a prebuilt library.
            Only applies to "cc_prebuilt_library" type atoms.
    """
    return verify_atom_is_in_allowlist(
        label = _get_idk_label(name),
        type = type,
        category = category,
        stable = stable,
        testonly = testonly,
        prebuilt_library_format = prebuilt_library_format,
    )

def verify_atom_is_in_allowlist(
        *,
        label,
        type,
        category,
        stable,
        testonly,
        prebuilt_library_format = None):
    """Verifies that the atom is in the allowlist for its type, category, and stability.

    If the atom fails verification, `fail()` is called with a message describing
    the issue. Otherwise, the function returns without side effects. No target
    is created.

    All atoms must be in an allowlist.

    Args:
        label: The Label of the atom to verify.
        type: The atom's type.
        category: The atom's category.
        stable: Whether the atom is stable.
        testonly: Standard meaning.
        prebuilt_library_format:  The format of a prebuilt library.
            Only applies to "cc_prebuilt_library" type atoms.
    """
    if bool(prebuilt_library_format) != (type == "cc_prebuilt_library"):
        fail("`prebuilt_library_format` must be set if and only if `type` is 'cc_prebuilt_library'.")

    # Strip the leading "@@" from the label string if present.
    label_str = str(label).lstrip("@")

    allowed_targets = None

    if type == "cc_source_library":
        if category == "partner":
            if stable:
                allowed_targets = STABLE_CC_SOURCE_LIBRARY_ATOMS
            else:
                allowed_targets = UNSTABLE_CC_SOURCE_LIBRARY_ATOMS
    elif type == "cc_prebuilt_library":
        if category == "partner" and stable:
            if prebuilt_library_format == "shared":
                allowed_targets = CC_PREBUILT_SHARED_LIBRARY_ATOMS
            elif prebuilt_library_format == "static":
                allowed_targets = CC_PREBUILT_STATIC_LIBRARY_ATOMS
            else:
                fail("Unrecognized prebuilt library format: '%s'" % prebuilt_library_format)
    elif type == "data":
        if category == "partner" and stable:
            allowed_targets = DATA_ATOMS
    elif type == "fidl_library":
        if category == "partner":
            if stable:
                allowed_targets = PARTNER_IDK_STABLE_FIDL_LIBRARY_ATOMS_LIST
            else:
                allowed_targets = PARTNER_IDK_UNSTABLE_FIDL_LIBRARY_ATOMS_LIST
        if category == "prebuilt" and stable:
            allowed_targets = PREBUILT_FIDL_LIBRARY_ATOMS_LIST
        elif category == "host_tool" and stable:
            allowed_targets = HOST_TOOL_FIDL_LIBRARY_ATOMS_LIST
        elif category == "compat_test" and stable:
            allowed_targets = COMPAT_TEST_FIDL_LIBRARY_ATOMS_LIST
    elif type == "host_tool":
        if category == "partner" and stable:
            allowed_targets = ALL_HOST_TOOL_ATOMS
    elif type == "none":
        if category == "partner" and stable:
            allowed_targets = NOOP_ATOMS_LIST
    else:
        fail("Unhandled atom type: '%s'" % type)

    if allowed_targets == None:
        fail(("No allowlist for type='%s', category='%s', stable='%s'. " +
              "Does target `%s` have the correct values? " +
              "Add a new allowlist when adding support for other categories or stability.") %
             (type, category, stable, label_str))

    # Exempt IDK test atoms from allowlist verification.
    # Do this last so they still exercise all the other logic.
    if label_str.startswith("//build/bazel/bazel_idk/tests:") and testonly:
        return

    # TODO(https://fxbug.dev/496597510): Consider optimizing performance by
    # making the lists sets or doing a binary search through the sorted lists.
    if label_str not in allowed_targets:
        fail("Target `%s` is not in the allowlist for type='%s', category='%s', stable='%s'" %
             (label_str, type, category, stable))

def get_atom_visibility(target_visibility, *, is_fidl_library = False):
    """Returns the visibility to use for an atom target.

    The atom's visibility should allow IDK contents/definition rules to depend
    on the atom in addition to the visibility specified to the macro.
    The built-in visibility labels cannot be used in combination with other
    labels so handle them specifically.

    Args:
        target_visibility: The visibility of the underlying target.
        is_fidl_library: Whether the atom is a FIDL library.

    Returns:
        A visibility list that also allows access by targets that define the IDK.
    """

    # TODO(https://fxbug.dev/431287514): Support package `default_visibility`.
    if "//visibility:public" in target_visibility:
        return target_visibility

    # All atoms must be visible to the targets defining the IDK.
    atom_visibility = ["//sdk/fidl:__pkg__" if is_fidl_library else "//sdk:__pkg__"]

    if "//visibility:private" not in target_visibility:
        atom_visibility += target_visibility

    return atom_visibility

def get_api_file_path(*, idk_name, stable, api_file_path):
    """Returns the string path to the API file for the given IDK atom.

    If `stable` is False, `api_file_path` must be `None` and `None` is returned.
    If `stable` is True and `api_file_path` is not specified, the returned path
    is "<idk_name>.api". Otherwise, `api_file_path` is returned.

    `api_file_path` must not specify the default path. This is verified when
    possible.

    Args:
        idk_name: String name of this atom within the IDK.
        stable: Whether this atom is stabilized.
        api_file_path: String path for the file representing the API exposed by
            this atom. Overrides the default path. Can be `None`.

    Returns:
        The string path to the API file for the given IDK target. May be `None`.
    """
    if stable:
        default_api_path = idk_name + ".api"
        if api_file_path:
            # Check that `api_file_path` does not specify the default path.
            # We must assume that absolute paths are not specifying the default
            # path because `relativize()` fails with absolute paths and we
            # cannot get the package path at this point.
            if not paths.is_absolute(api_file_path):
                if paths.relativize(api_file_path, ".") == paths.relativize(default_api_path, "."):
                    fail("The specified `api_file_path` (`%s`) matches the default. `api_file_path` only needs to be specified when overriding the default." % api_file_path)

            return api_file_path
        else:
            return default_api_path
    else:
        if api_file_path != None:
            fail("`api_file_path` must only be specified for stable IDK atoms.")
        return None

def _get_golden_file_path_for_api_level(golden_file_name, api_level):
    """Returns the path to the specified golden file for the specified API level.

    The paths are in the format of absolute labels (with leading slashes and a
    colon) except for the "PLATFORM" API level where the path is just a file name.

    Args:
        golden_file_name: The name of the golden file in each API level.
        api_level: The API level for which to get the golden file path.

    Returns:
        The path to the golden file for the specified API level.
    """
    if api_level == "PLATFORM":
        return golden_file_name
    else:
        return "//sdk/history/" + api_level + ":" + golden_file_name

def _get_api_level_condition(api_level):
    """Returns the label for the condition that is true when the current API level is `api_level`.

    Args:
        api_level: The API level for which to get the condition.

    Returns:
        The label for the condition that is true when the current API level is `api_level`.
    """
    return "//build/bazel/versioning:is_api_level_" + api_level

def get_golden_file(golden_file_name, *, support_platform = False):
    """Returns a `select()` statement for the golden file for the current API level.

    The `select()` statement maps each API level to the label for the
    `golden_file_name` file at each IDK buildable API level.

    Args:
        golden_file_name: The name of the golden file in each API level.
        support_platform: Whether to include an entry for the "PLATFORM" API
            level in the `select()` statement. If True, the file is assumed to
            be located in the same directory as the `BUILD.bazel` file.

    Returns:
        A `select()` statement for the golden file for the current API level.
    """
    api_levels = list(idk_buildable_api_levels)
    if support_platform:
        api_levels.append("PLATFORM")

    level_map = {}
    for api_level in api_levels:
        level_map[_get_api_level_condition(api_level)] = _get_golden_file_path_for_api_level(golden_file_name, api_level)

    return select(level_map)
