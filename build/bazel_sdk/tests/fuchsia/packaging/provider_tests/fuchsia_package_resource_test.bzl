# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@bazel_skylib//lib:unittest.bzl", "analysistest", "asserts")
load("@rules_fuchsia//fuchsia:defs.bzl", "fuchsia_find_all_package_resources", "fuchsia_package_resource", "fuchsia_package_resource_collection")
load("@rules_fuchsia//fuchsia/private:providers.bzl", "FuchsiaCollectedPackageResourcesInfo", "FuchsiaPackageResourcesInfo")

## Provider Tests

def _provider_contents_test_impl(ctx):
    env = analysistest.begin(ctx)

    target_under_test = analysistest.target_under_test(env)
    resources = target_under_test[FuchsiaPackageResourcesInfo].resources

    # make sure all counts are the same
    asserts.equals(env, len(ctx.attr.expected_dests), len(ctx.attr.expected_srcs))
    asserts.equals(env, len(ctx.attr.expected_dests), len(resources))

    for idx in range(0, len(resources)):
        resource = resources[idx]

        # verify that our src is set correctly
        asserts.equals(
            env,
            ctx.attr.expected_srcs[idx],
            resource.src.path,
        )

        # verify that the dest is set correctly
        asserts.equals(
            env,
            ctx.attr.expected_dests[idx],
            resource.dest,
        )

    return analysistest.end(env)

provider_contents_test = analysistest.make(
    _provider_contents_test_impl,
    attrs = {
        "expected_dests": attr.string_list(),
        "expected_srcs": attr.string_list(),
    },
)

def _test_provider_contents():
    expected_dest = "/data/foo"

    # Note, the expected_src needs to be in sync with the location of this file.
    expected_src = "fuchsia/packaging/provider_tests/text_file.txt"

    # Rule under test.
    fuchsia_package_resource(
        name = "provider_contents_subject",
        dest = expected_dest,
        src = ":text_file.txt",
        tags = ["manual"],
    )

    # Testing rule.
    provider_contents_test(
        name = "provider_contents_test",
        target_under_test = ":provider_contents_subject",
        expected_dests = [expected_dest],
        expected_srcs = [expected_src],
    )

def _test_multiple_resources():
    expected_dests = ["/data/foo", "/data/bar"]

    # Note, the expected_src needs to be in sync with the location of this file.
    expected_src = "fuchsia/packaging/provider_tests/text_file.txt"

    fuchsia_package_resource(
        name = "resource_1",
        dest = expected_dests[0],
        src = ":text_file.txt",
        tags = ["manual"],
    )

    fuchsia_package_resource(
        name = "resource_2",
        dest = expected_dests[1],
        src = ":text_file.txt",
        tags = ["manual"],
    )

    # Rule under test.
    fuchsia_package_resource_collection(
        name = "multi_provider_contents_subject",
        resources = [":resource_1", ":resource_2"],
        tags = ["manual"],
    )

    # Testing rule.
    provider_contents_test(
        name = "multi_provider_contents_test",
        target_under_test = ":multi_provider_contents_subject",
        expected_dests = expected_dests,
        expected_srcs = [expected_src, expected_src],
    )

## Failure Tests

def _failure_testing_test_impl(ctx):
    env = analysistest.begin(ctx)
    asserts.expect_failure(env, ctx.attr.expected_failure_message)
    return analysistest.end(env)

failure_testing_test = analysistest.make(
    _failure_testing_test_impl,
    expect_failure = True,
    attrs = {
        "expected_failure_message": attr.string(),
    },
)

def _test_empty_dest_failure():
    fuchsia_package_resource(
        name = "empty_dest_should_fail",
        dest = "",
        src = ":text_file.txt",
        tags = ["manual"],
    )

    failure_testing_test(
        name = "empty_dest_should_fail_test",
        target_under_test = ":empty_dest_should_fail",
        expected_failure_message = "dest must not be an empty string",
    )

## Resource Collection Tests
def _resource_collection_test_impl(ctx):
    env = analysistest.begin(ctx)

    target_under_test = analysistest.target_under_test(env)
    collected_dests = [
        r.dest
        for r in target_under_test[FuchsiaCollectedPackageResourcesInfo].collected_resources.to_list()
    ]
    expected_dests = ctx.attr.expected_dests

    asserts.equals(env, sorted(collected_dests), sorted(expected_dests))

    return analysistest.end(env)

resource_collection_test = analysistest.make(
    _resource_collection_test_impl,
    attrs = {
        "collected_resources": attr.label(
            doc = "The result of collecting all of the resources",
        ),
        "expected_dests": attr.string_list(
            doc = """A list of strings representing expected destinations where
            resources will be installed""",
        ),
    },
)

def _target_that_creates_a_resource_impl(ctx):
    f = ctx.actions.declare_file(ctx.label.name + "_file.txt")
    ctx.actions.write(f, content = "")
    return [
        DefaultInfo(
            files = depset([f]),
        ),
        FuchsiaPackageResourcesInfo(
            resources = [struct(
                src = f,
                dest = ctx.attr.dest,
            )],
        ),
    ]

_target_that_creates_a_resource = rule(
    implementation = _target_that_creates_a_resource_impl,
    attrs = {
        "dest": attr.string(),
    },
)

def _target_with_resource_deps_impl(ctx):
    f = ctx.actions.declare_file(ctx.label.name + "_file.txt")
    ctx.actions.write(f, content = "")
    return [
        DefaultInfo(
            files = depset([f]),
        ),
    ]

_target_with_resource_deps = rule(
    implementation = _target_with_resource_deps_impl,
    attrs = {
        "deps": attr.label_list(),
    },
)

def _test_resource_collection_contents():
    expected_dests = ["/data/a", "/data/b", "/data/c"]

    _target_that_creates_a_resource(
        name = "resource_collection_target_create_resource",
        dest = expected_dests[0],
        tags = ["manual"],
    )

    fuchsia_package_resource(
        name = "dep_for_target_with_deps",
        dest = expected_dests[1],
        src = ":text_file.txt",
        tags = ["manual"],
    )

    _target_with_resource_deps(
        name = "resource_collection_target_with_deps",
        deps = [
            ":dep_for_target_with_deps",
        ],
        tags = ["manual"],
    )

    fuchsia_package_resource(
        name = "resource_collection_from_rule",
        dest = expected_dests[2],
        src = ":text_file.txt",
        tags = ["manual"],
    )

    found_resources = "find_all_resources"
    fuchsia_find_all_package_resources(
        name = found_resources,
        deps = [
            ":resource_collection_target_create_resource",
            ":resource_collection_target_with_deps",
            ":resource_collection_from_rule",
        ],
        tags = ["manual"],
    )

    # Testing rule.
    resource_collection_test(
        name = "resource_collection_test",
        target_under_test = ":find_all_resources",
        expected_dests = expected_dests,
    )

# Entry point from the BUILD file; macro for running each test case's macro and
# declaring a test suite that wraps them together.
# buildifier: disable=function-docstring
def fuchsia_package_resource_test_suite(name, **kwargs):
    # Call all test functions and wrap their targets in a suite.
    _test_provider_contents()
    _test_empty_dest_failure()
    _test_multiple_resources()
    _test_resource_collection_contents()

    native.test_suite(
        name = name,
        tests = [
            ":provider_contents_test",
            ":multi_provider_contents_test",
            ":empty_dest_should_fail_test",
            ":resource_collection_test",
        ],
        **kwargs
    )
