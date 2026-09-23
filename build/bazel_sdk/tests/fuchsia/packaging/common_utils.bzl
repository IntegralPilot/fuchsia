# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load(
    "@rules_fuchsia//fuchsia/private:fuchsia_api_level.bzl",
    "FUCHSIA_API_LEVEL_TARGET",
    "REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET",
)

def _failure_test_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.expected_failure_message)
    return analysistest.end(env)

failure_test = analysistest.make(
    _failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
)

no_repo_default_api_level_failure_test = analysistest.make(
    _failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
    config_settings = {
        REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET: "",
    },
)

unknown_repo_default_api_level_failure_test = analysistest.make(
    _failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
    config_settings = {
        REPOSITORY_DEFAULT_FUCHSIA_API_LEVEL_TARGET: "98765",
    },
)

unknown_override_api_level_failure_test = analysistest.make(
    _failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
    config_settings = {
        FUCHSIA_API_LEVEL_TARGET: "123456",
    },
)
