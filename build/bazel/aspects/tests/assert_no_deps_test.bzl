# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load("//build/bazel/aspects:assert_no_deps.bzl", "TransitiveDepsInfo", "assert_no_deps_aspect")

def _dummy_lib_impl(_ctx):
    pass

dummy_lib = rule(
    implementation = _dummy_lib_impl,
    attrs = {
        "deps": attr.label_list(),
    },
)

def _assert_no_deps_success_test_impl(ctx):
    env = analysistest.begin(ctx)
    target = analysistest.target_under_test(env)
    asserts.true(env, TransitiveDepsInfo in target, "Expected TransitiveDepsInfo on target under test.")
    if env.failures:
        # Call fail explicitly so it fails on `bazel build`, which is needed to execute
        # these tests as build-time tests in our GN/Bazel setup today.
        fail("\n".join(env.failures))
    return analysistest.end(env)

assert_no_deps_success_test = analysistest.make(
    _assert_no_deps_success_test_impl,
    extra_target_under_test_aspects = [assert_no_deps_aspect],
)

def _assert_no_deps_failure_test_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.expected_message)
    if env.failures:
        # Call fail explicitly so it fails on `bazel build`, which is needed to execute
        # these tests as build-time tests in our GN/Bazel setup today.
        fail("\n".join(env.failures))
    return analysistest.end(env)

assert_no_deps_failure_test = analysistest.make(
    _assert_no_deps_failure_test_impl,
    expect_failure = True,
    extra_target_under_test_aspects = [assert_no_deps_aspect],
    attrs = {
        "expected_message": attr.string(mandatory = True),
    },
)

def assert_no_deps_test_suite(name):
    dummy_lib(
        name = "foo",
    )

    dummy_lib(
        name = "bar",
        deps = [":foo"],
    )

    dummy_lib(
        name = "valid_target",
        deps = [":bar"],
    )

    assert_no_deps_success_test(
        name = "success_test",
        target_under_test = ":valid_target",
    )

    dummy_lib(
        name = "direct_deps",
        deps = [":foo"],
        tags = ["assert_no_deps=:foo"],
    )

    assert_no_deps_failure_test(
        name = "direct_deps_failure_test",
        target_under_test = ":direct_deps",
        expected_message = "direct_deps violates assert_no_deps: found forbidden dependencies: :foo",
    )

    dummy_lib(
        name = "indirect_deps",
        deps = [":bar"],
        tags = ["assert_no_deps=:foo"],
    )

    assert_no_deps_failure_test(
        name = "transitive_deps_failure_test",
        target_under_test = ":indirect_deps",
        expected_message = "indirect_deps violates assert_no_deps: found forbidden dependencies: :foo",
    )

    dummy_lib(
        name = "tag_on_deps",
        deps = [":direct_deps"],
    )

    assert_no_deps_failure_test(
        name = "tag_on_deps_failure_test",
        target_under_test = ":tag_on_deps",
        expected_message = "direct_deps violates assert_no_deps: found forbidden dependencies: :foo",
    )

    dummy_lib(
        name = "absolute_deps",
        deps = ["//build/bazel/aspects/tests:foo"],
        tags = ["assert_no_deps=:foo"],
    )

    assert_no_deps_failure_test(
        name = "absolute_deps_failure_test",
        target_under_test = ":absolute_deps",
        expected_message = "absolute_deps violates assert_no_deps: found forbidden dependencies: :foo",
    )

    dummy_lib(
        name = "absolute_tags",
        deps = [":foo"],
        tags = ["assert_no_deps=//build/bazel/aspects/tests:foo"],
    )

    assert_no_deps_failure_test(
        name = "absolute_tags_failure_test",
        target_under_test = ":absolute_tags",
        expected_message = "absolute_tags violates assert_no_deps: found forbidden dependencies: //build/bazel/aspects/tests:foo",
    )

    dummy_lib(
        name = "absolute_dep_and_tag",
        deps = ["//build/bazel/aspects/tests:foo"],
        tags = ["assert_no_deps=//build/bazel/aspects/tests:foo"],
    )

    assert_no_deps_failure_test(
        name = "absolute_dep_and_tag_failure_test",
        target_under_test = ":absolute_dep_and_tag",
        expected_message = "absolute_dep_and_tag violates assert_no_deps: found forbidden dependencies: //build/bazel/aspects/tests:foo",
    )

    dummy_lib(
        name = "pkg_deps",
        deps = [":foo"],
        tags = ["assert_no_deps=:__pkg__"],
    )

    assert_no_deps_failure_test(
        name = "pkg_deps_failure_test",
        target_under_test = ":pkg_deps",
        expected_message = "pkg_deps violates assert_no_deps: found forbidden dependencies: :__pkg__",
    )

    dummy_lib(
        name = "absolute_pkg_tags",
        deps = [":foo"],
        tags = ["assert_no_deps=//build/bazel/aspects/tests:__pkg__"],
    )

    assert_no_deps_failure_test(
        name = "absolute_pkg_tags_failure_test",
        target_under_test = ":absolute_pkg_tags",
        expected_message = "absolute_pkg_tags violates assert_no_deps: found forbidden dependencies: //build/bazel/aspects/tests:__pkg__",
    )

    dummy_lib(
        name = "pkg_success",
        deps = [":foo"],
        tags = ["assert_no_deps=//build/bazel/aspects:__pkg__"],
    )

    assert_no_deps_success_test(
        name = "pkg_success_test",
        target_under_test = ":pkg_success",
    )

    dummy_lib(
        name = "subpackages_deps",
        deps = [":foo"],
        tags = ["assert_no_deps=//build/bazel/aspects:__subpackages__"],
    )

    assert_no_deps_failure_test(
        name = "subpackages_deps_failure_test",
        target_under_test = ":subpackages_deps",
        expected_message = "subpackages_deps violates assert_no_deps: found forbidden dependencies: //build/bazel/aspects:__subpackages__",
    )

    dummy_lib(
        name = "local_subpackages_deps",
        deps = [":foo"],
        tags = ["assert_no_deps=:__subpackages__"],
    )

    assert_no_deps_failure_test(
        name = "local_subpackages_deps_failure_test",
        target_under_test = ":local_subpackages_deps",
        expected_message = "local_subpackages_deps violates assert_no_deps: found forbidden dependencies: :__subpackages__",
    )

    dummy_lib(
        name = "subpackages_success",
        deps = [":foo"],
        tags = ["assert_no_deps=//build/bazel/rules:__subpackages__"],
    )

    assert_no_deps_success_test(
        name = "subpackages_success_test",
        target_under_test = ":subpackages_success",
    )

    native.test_suite(
        name = name,
        visibility = ["//visibility:public"],
        tests = [
            ":success_test",
            ":direct_deps_failure_test",
            ":transitive_deps_failure_test",
            ":tag_on_deps_failure_test",
            ":absolute_deps_failure_test",
            ":absolute_tags_failure_test",
            ":absolute_dep_and_tag_failure_test",
            ":pkg_deps_failure_test",
            ":absolute_pkg_tags_failure_test",
            ":pkg_success_test",
            ":subpackages_deps_failure_test",
            ":local_subpackages_deps_failure_test",
            ":subpackages_success_test",
        ],
    )
