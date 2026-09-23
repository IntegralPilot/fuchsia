# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Define the json_validator_valico_test() macro."""

load("//build/bazel/rules/host_tests:host_py_test.bzl", "host_py_test")

def json_validator_valico_test(name, test_schema, test_document, extra_args = [], expect_failure = False):
    """Define a single test target that runs json_validator_valico.

    This will run: json_validator_valico <test_schema> <test_document>
    and verify that it succeeded, or failed if expect_failure==True.

    Args:
        test_schema: Label to the test schema to use as input.
        test_document: Label to the test document to use as input.
        extra_args: A list of extra command-line arguments passed to the command.
        expect_failure: True if failure is expected.
    """

    # IMPLEMENTATION NOTE: It is easier to pass the rlocation path to json_validator_valico
    # as a test argument, instead of hard-coding it in the test runner target.
    json_validator_valico = "//build/tools/json_validator:json_validator_valico"

    host_py_test(
        name = name,
        main = "json_validator_valico_test_runner.py",
        srcs = ["json_validator_valico_test_runner.py"],
        deps = [
            "@rules_python//python/runfiles",
        ],
        test_args = (["--expect-failure"] if expect_failure else []) + [
            "$(rlocationpath {})".format(json_validator_valico),
            "$(rlocationpath {})".format(test_schema),
            "$(rlocationpath {})".format(test_document),
        ] + extra_args,
        test_data = [
            json_validator_valico,
            test_schema,
            test_document,
        ],
        visibility = ["//visibility:public"],
    )
