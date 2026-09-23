# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-tests for transition_utils.bzl."""

load("@bazel_skylib//lib:unittest.bzl", "asserts", "unittest")
load(
    "//common:transition_utils.bzl",
    "normalize_features_list",
    "remove_command_line_option_values",
    "set_command_line_option_value",
)

def _test_set_command_line_option_value(env):
    cases = [
        (["foo1"], ["foo1", "--arg=new"]),
        (["foo2", "--arg=new"], ["foo2", "--arg=new"]),
        (["foo3", "--arg=old"], ["foo3", "--arg=new"]),
        (["--arg=old", "foo4", "--arg=new"], ["--arg=new", "foo4", "--arg=new"]),
    ]
    for input, expected in cases:
        asserts.equals(env, expected, set_command_line_option_value(input, "--arg=", "new"))

def _test_remove_command_line_option_values(env):
    cases = [
        (["foo5"], ["foo5"]),
        (["foo6", "--arg=new"], ["foo6"]),
        (["--arg=old", "foo7", "--arg=new", "bar"], ["foo7", "bar"]),
    ]
    for input, expected in cases:
        asserts.equals(env, expected, remove_command_line_option_values(input, "--arg="))

def _test_normalize_features_list(env):
    cases = [
        (["foo"], ["foo"]),
        (["foo", "bar"], ["foo", "bar"]),
        (["foo", "foo"], ["foo"]),
        (["-foo", "-foo"], ["-foo"]),
        (["foo", "-foo"], ["-foo"]),
        (["foo", "-foo", "foo"], ["foo"]),
        (["foo", "-foo", "foo", "-foo"], ["-foo"]),
        (["foo", "bar", "-foo"], ["bar", "-foo"]),
        (["-foo", "bar", "foo", "-bar"], ["foo", "-bar"]),
        (["-foo", "bar", "-bar", "foo"], ["-bar", "foo"]),
    ]
    for input, expected in cases:
        asserts.equals(env, expected, normalize_features_list(input))

def _transition_utils_test(ctx):
    env = unittest.begin(ctx)
    _test_normalize_features_list(env)
    _test_set_command_line_option_value(env)
    _test_remove_command_line_option_values(env)
    return unittest.end(env)

transition_utils_test = unittest.make(_transition_utils_test)
