# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for the utils.bzl file"""

load("@bazel_skylib//lib:unittest.bzl", "asserts", "unittest")
load("@rules_fuchsia//fuchsia/private:utils.bzl", "append_suffix_to_label")

def _append_suffix_to_label_test(ctx):
    env = unittest.begin(ctx)

    if ctx.attr.separator:
        new_value = append_suffix_to_label(ctx.attr.base_name, ctx.attr.value_to_append, separator = ctx.attr.separator)
    else:
        new_value = append_suffix_to_label(ctx.attr.base_name, ctx.attr.value_to_append)

    # Assert statements go here
    asserts.equals(
        env,
        new_value,
        ctx.attr.expected_value,
    )

    return unittest.end(env)

append_suffix_to_label_test = unittest.make(
    _append_suffix_to_label_test,
    attrs = {
        "base_name": attr.string(doc = "The name to test", mandatory = True),
        "value_to_append": attr.string(doc = "The value to append", mandatory = True),
        "separator": attr.string(doc = "The string separator", mandatory = False),
        "expected_value": attr.string(doc = "What we should get", mandatory = True),
    },
)

# Entry point for the BUILD file
def append_suffix_to_label_test_suite(name, **kwargs):
    """ The test suite for the append_suffix_to_label utility function

    Args:
        name: The name of the test suite.
        **kwargs: The args to pass to the test_suite target.
    """
    append_suffix_to_label_test(
        name = "no_colon_no_separator",
        base_name = "foo",
        value_to_append = "bar",
        expected_value = "foo.bar",
    )

    append_suffix_to_label_test(
        name = "no_colon_separator",
        base_name = "foo",
        value_to_append = "bar",
        separator = "_",
        expected_value = "foo_bar",
    )

    append_suffix_to_label_test(
        name = "colon_no_separator",
        base_name = ":foo",
        value_to_append = "bar",
        expected_value = ":foo.bar",
    )

    append_suffix_to_label_test(
        name = "colon_separator",
        base_name = ":foo",
        value_to_append = "bar",
        separator = "_",
        expected_value = ":foo_bar",
    )

    append_suffix_to_label_test(
        name = "full_path_no_colon",
        base_name = "//src/foo",
        value_to_append = "bar",
        expected_value = "//src/foo:foo.bar",
    )

    append_suffix_to_label_test(
        name = "full_path_with_colon_different_name",
        base_name = "//src:foo",
        value_to_append = "bar",
        expected_value = "//src:foo.bar",
    )

    append_suffix_to_label_test(
        name = "full_path_with_colon_same_name",
        base_name = "//src/foo:foo",
        value_to_append = "bar",
        expected_value = "//src/foo:foo.bar",
    )

    native.test_suite(
        name = name,
        tests = [
            ":no_colon_no_separator",
            ":no_colon_separator",
            ":colon_no_separator",
            ":colon_separator",
            ":full_path_no_colon",
            ":full_path_with_colon_different_name",
            ":full_path_with_colon_same_name",
        ],
        **kwargs
    )
