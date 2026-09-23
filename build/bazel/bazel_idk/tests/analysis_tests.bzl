# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Analysis tests for IDK macros."""

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load("//build/bazel/rules/idk/private:idk_atom.bzl", "idk_atom")

def _failure_test_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.expected_message)
    return analysistest.end(env)

# Defines a test that expects the target under test to fail analysis with the specified error.
failure_test = analysistest.make(
    _failure_test_impl,
    expect_failure = True,
    attrs = {
        "expected_message": attr.string(mandatory = True),
    },
)

def analysis_test_suite(
        name,
        visibility = None):
    """Defines analysis tests for the IDK macros.

    Args:
        name: Name of the test suite.
        visibility: Visibility of the test suite.
    """

    # For these specific test atoms, `testonly` is False to bypass the allowlist
    # exemption for atoms in this Bazel package.

    # An atom that is not in the appropriate allowlist.
    idk_atom(
        name = "test_atom_not_in_allowlist_idk",
        testonly = False,
        api_area = "Developer",
        category = "partner",
        id = "sdk://pkg/test_not_in_allowlist",
        idk_name = "test_not_in_allowlist",
        meta_dest = "/pkg/test_not_in_allowlist",
        stable = True,
        type = "data",
        target_compatible_with = ["@platforms//os:fuchsia"],
        tags = ["manual"],
    )

    failure_test(
        name = "not_in_allowlist_failure_test",
        target_under_test = ":test_atom_not_in_allowlist_idk",
        expected_message = "Target `//build/bazel/bazel_idk/tests:test_atom_not_in_allowlist_idk` is not in the allowlist for type='data', category='partner', stable='True'",
        size = "small",
    )

    # An atom configuration for which there is no corresponding allowlist.
    idk_atom(
        name = "test_atom_no_allowlist_idk",
        testonly = False,
        api_area = "Developer",
        category = "prebuilt",  # There is no allowlist for prebuilt data atoms.
        id = "sdk://pkg/test_no_allowlist",
        idk_name = "test_no_allowlist",
        meta_dest = "/pkg/test_no_allowlist",
        stable = True,
        type = "data",
        target_compatible_with = ["@platforms//os:fuchsia"],
        tags = ["manual"],
    )

    failure_test(
        name = "no_allowlist_failure_test",
        target_under_test = ":test_atom_no_allowlist_idk",
        expected_message = "No allowlist for type='data', category='prebuilt', stable='True'. Does target `//build/bazel/bazel_idk/tests:test_atom_no_allowlist_idk` have the correct values? Add a new allowlist when adding support for other categories or stability.",
        size = "small",
    )

    # NOTE: We cannot test cases of attribute combinations that do not have an
    # allowlist for most atom types because their macros call
    # `verify_target_is_in_allowlist()`, causing loading to fail.

    native.test_suite(
        name = name,
        tests = [
            ":not_in_allowlist_failure_test",
            ":no_allowlist_failure_test",
        ],
        visibility = visibility,
    )
