# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" Defines utilities for working with fuchsia api levels. """

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")

# NOTE: INTERNAL_ONLY_ALL_KNOWN_API_LEVELS is part of the generated content of @fuchsia_sdk
# and does not exist in @rules_fuchsia.
load("@fuchsia_sdk//:api_version.bzl", "INTERNAL_ONLY_SUPPORTED_API_LEVELS")

# We define the provider in this file because it is a private implementation
# detail in this file. It is only made public so that it can be used in tests.
FuchsiaAPILevelInfo = provider(
    "Specifies what api to use while building",
    fields = {
        "level": "The API level",
    },
)

# The resolved canonical label name for the API level target.
# Prefer to resolve the entire label instead of just replacing @fuchsia_sdk
# to make copybara transforms easier.
FUCHSIA_API_LEVEL_TARGET = str(Label(
    "@fuchsia_sdk//flags:fuchsia_api_level",
))
REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET = str(Label(
    "@fuchsia_sdk//flags:repository_default_fuchsia_api_level",
))

# Rules that require the fuchsia api level should depend on this attribute set.
# They can then use the helper functions in this file to get the flags needed.
FUCHSIA_API_LEVEL_ATTRS = {
    "_fuchsia_api_level": attr.label(
        default = FUCHSIA_API_LEVEL_TARGET,
    ),
}

def get_fuchsia_api_levels():
    """ Returns the list of API levels supported by this SDK version.

    Values are returned as a struct with the following fields:
    struct(
        abi_revision = "0xED74D73009C2B4E3",
        api_level = "10",
        as_u32 = 10,
    )

    `as_u32` is interesting in the case of special API levels like `HEAD`.
    clang only wants to be passed API levels in their numeric form.

    The status is not an API to be relied on but the STATUS_* constants can be
    used.
    """
    return INTERNAL_ONLY_SUPPORTED_API_LEVELS

def get_fuchsia_api_level(ctx):
    """ Returns the raw api level to use for building.

    When using this function in a rule, add `FUCHSIA_API_LEVEL_ATTRS` to its `attrs`.

    Must only be called within the scope of `fuchsia_transition` or some other
    context where FUCHSIA_API_LEVEL_TARGET has been set appropriately,
    including considering all ways to set the API level as appropriate.

    Args:
        ctx: A rule context object.

    Returns:
        A string containing the valid API level in FUCHSIA_API_LEVEL_TARGET.
    """
    return ctx.attr._fuchsia_api_level[FuchsiaAPILevelInfo].level

def _valid_api_level_names():
    """ Returns a list of strings containing the names of the API levels supported by the SDK.
    """

    # The returned list is sorted alphabetically, which is not reader-friendly.
    return [entry.api_level for entry in get_fuchsia_api_levels()]

def _info_for_fuchsia_api_level_or_none(api_level):
    """ Get a struct containg infor about `api_level` if known.

    Args:
        api_level: A string containing the API level name.

    Returns:
        A struct containg infor about `api_level` or `None` if `api_level` is
        not known to this SDK version.
    """
    fuchsia_api_level_infos = [
        info
        for info in get_fuchsia_api_levels()
        if info.api_level == api_level
    ]
    if len(fuchsia_api_level_infos) > 1:
        fail("Assertion failure: There should be exactly one info for a supported level. Found: ", fuchsia_api_level_infos)
    return fuchsia_api_level_infos[0] if fuchsia_api_level_infos else None

def u32_for_fuchsia_api_level(api_level):
    """ Get the integer representation of `api_level`.

    Args:
        api_level: A string containing the API level name. It must be known to
        this SDK version.

    Returns:
         The integer representation of `api_level`.
    """
    info = _info_for_fuchsia_api_level_or_none(api_level)
    if not info:
        fail('API level "%s" is unrecognized.' % api_level)
    return info.as_u32

def u32_for_fuchsia_api_level_or_none(api_level):
    """ Get the integer representation of `api_level` if known.

    For use during transition when errors are not desirable.

    Args:
        api_level: A string containing the API level name.

    Returns:
        The integer representation of `api_level` or `None` if
        `api_level` is not known to this SDK version.

    """
    info = _info_for_fuchsia_api_level_or_none(api_level)
    return info.as_u32 if info else None

def _fuchsia_api_level_impl(ctx):
    raw_level = ctx.build_setting_value

    # Only validate targets if fuchsia_targets_enabled is true. The fuchsia_targets_enabled flag
    # defaults to true and is only enabled in repositories which have infrastructure settings that
    # require it.
    if ctx.attr._fuchsia_targets_enabled_flag[BuildSettingInfo].value:
        if raw_level == "":
            # All we know is that this rule is being analyzed with the level set to the empty string,
            # which is the default, and the label of the rule. We do not know why the rule is being
            # analyzed, though most likely it is FUCHSIA_API_LEVEL_TARGET being analyzed for a
            # target after `fuchsia_transition`, meaning none of the API level mechanisms were set.
            fail("ERROR: An API level has not been specified for this target or the repository.")

        if raw_level not in _valid_api_level_names():
            fail('ERROR: "{}" is not an API level supported by this SDK. API level should be one of {}.'.format(
                raw_level,
                _valid_api_level_names(),
            ))

    return FuchsiaAPILevelInfo(
        level = raw_level,
    )

fuchsia_api_level = rule(
    doc = """A build configuration value containing the fuchsia api level

    The fuchsia_api_level is a build configuration value that can be set from
    the command line. This lets users define how they what api level they want
    to use outside of a BUILD.bazel file.
    """,
    implementation = _fuchsia_api_level_impl,
    build_setting = config.string(flag = True),
    attrs = {
        "_fuchsia_targets_enabled_flag": attr.label(doc = """
        A flag that signals that we are not building fuchsia. This is needed
        so that we can skip checking the API level during analysis for builds
        like bazel build //... which might analyze fuchsia targets that depend
        on the api level flag but depend on the fuchsia_transition to set it.
        """, default = "@rules_fuchsia//fuchsia/flags:fuchsia_targets_enabled"),
    },
)

def _verify_cc_head_api_level_impl(ctx):
    f = ctx.actions.declare_file(ctx.label.name + "__verify_head_api_level.cc")
    ctx.actions.write(f, content = "")

    if get_fuchsia_api_level(ctx) != "HEAD":
        fail("You are trying to use an unstable API in a stable package.\n" +
             "You must target \"HEAD\" in order to use this library: " + ctx.attr.library_name)

    return DefaultInfo(
        files = depset([f]),
    )

verify_cc_head_api_level = rule(
    implementation = _verify_cc_head_api_level_impl,
    doc = """Forces an unstable cc_library to only be used if targeting HEAD

    This rule should only be used by the generated cc_library rules for sdk
    elements. It will create an empty c++ file which can be added to the srcs
    of the cc_library. The check will look at the Fuchsia API level and fail if
    it is not "HEAD".
    """,
    attrs = {
        "library_name": attr.string(mandatory = True),
        "_fuchsia_api_level": attr.label(
            # We have to explicitly depend on the @fuchsia_sdk target because
            # this is used from the internal_sdk as well.
            default = FUCHSIA_API_LEVEL_TARGET,
        ),
    },
)
