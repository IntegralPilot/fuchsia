# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia API level support."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load("@rules_fuchsia//fuchsia/private:fuchsia_api_level.bzl", "FuchsiaAPILevelInfo", "fuchsia_api_level")
load("//test_utils:api_levels.bzl", "some_valid_numerical_api_level_as_string")

def _level_setting_test_impl(ctx):
    env = analysistest.begin(ctx)

    target_under_test = analysistest.target_under_test(env)
    api_level_info = target_under_test[FuchsiaAPILevelInfo]

    asserts.equals(
        env,
        ctx.attr.expected_level,
        api_level_info.level,
    )

    return analysistest.end(env)

level_setting_test = analysistest.make(
    _level_setting_test_impl,
    attrs = {
        "expected_level": attr.string(),
    },
)

level_setting_test_no_checks_test = analysistest.make(
    _level_setting_test_impl,
    attrs = {
        "expected_level": attr.string(),
    },
    config_settings = {
        "@@rules_fuchsia+//fuchsia/flags:fuchsia_targets_enabled": False,
    },
)

def _make_test_fuchsia_api_level(name, level):
    fuchsia_api_level(
        name = name,
        build_setting_default = level,
        target_compatible_with = ["@platforms//os:fuchsia"],
    )

def _test_level_setting():
    _make_test_fuchsia_api_level(
        name = "supported_numerical_string",
        level = some_valid_numerical_api_level_as_string(),
    )

    level_setting_test(
        name = "test_setting_supported_numerical_string",
        target_under_test = ":supported_numerical_string",
        expected_level = some_valid_numerical_api_level_as_string(),
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "next",
        level = "NEXT",
    )

    level_setting_test(
        name = "test_setting_next",
        target_under_test = ":next",
        expected_level = "NEXT",
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "head",
        level = "HEAD",
    )

    level_setting_test(
        name = "test_setting_head",
        target_under_test = ":head",
        expected_level = "HEAD",
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "unset",
        level = "",
    )

    level_setting_failure_test(
        name = "test_unset",
        target_under_test = ":unset",
        expected_failure_message = "ERROR: An API level has not been specified for this target or the repository.",
        tags = ["manual"],
    )

    # test that we can skip the validation
    level_setting_test_no_checks_test(
        name = "test_no_validation_empty_string",
        target_under_test = "unset",
        expected_level = "",
    )

# Failure tests
def _level_setting_failure_test_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.expected_failure_message)
    return analysistest.end(env)

level_setting_failure_test = analysistest.make(
    _level_setting_failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
)

def _test_level_setting_failures():
    _make_test_fuchsia_api_level(
        name = "unknown_string",
        level = "SOMETHING",
    )

    level_setting_failure_test(
        name = "test_setting_unknown_string",
        target_under_test = ":unknown_string",
        expected_failure_message = '"SOMETHING" is not an API level supported by this SDK. API level should be one of ["',
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "retired_level",
        level = "21",
    )

    level_setting_failure_test(
        name = "test_retired",
        target_under_test = ":retired_level",
        expected_failure_message = '"21" is not an API level supported by this SDK. API level should be one of ["',
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "future_numerical_string",
        level = "90000",
    )

    level_setting_failure_test(
        name = "test_future_numerical_string",
        target_under_test = ":future_numerical_string",
        expected_failure_message = '"90000" is not an API level supported by this SDK. API level should be one of ["',
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "next_lowercase",
        level = "next",
    )

    level_setting_failure_test(
        name = "test_setting_next_lowercase",
        target_under_test = ":next_lowercase",
        expected_failure_message = '"next" is not an API level supported by this SDK. API level should be one of ["',
        tags = ["manual"],
    )

    _make_test_fuchsia_api_level(
        name = "platform",
        level = "PLATFORM",
    )

    level_setting_failure_test(
        name = "test_setting_platform",
        target_under_test = ":platform",
        expected_failure_message = '"PLATFORM" is not an API level supported by this SDK. API level should be one of ["',
        tags = ["manual"],
    )

def fuchsia_api_level_test_suite(name, **kwargs):
    _test_level_setting()
    _test_level_setting_failures()

    native.test_suite(
        name = name,
        tests = [
            # _test_level_setting tests
            ":test_setting_supported_numerical_string",
            ":test_setting_next",
            ":test_setting_head",
            ":test_setting_platform",
            ":test_unset",
            ":test_no_validation_empty_string",

            # _test_level_setting_failures tests
            ":test_setting_unknown_string",
            ":test_retired",
            ":test_future_numerical_string",
            ":test_setting_next_lowercase",
        ],
        **kwargs
    )
