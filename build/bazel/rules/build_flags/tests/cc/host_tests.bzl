# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//build_flags:cc.bzl",
    "wrap_cc_macro_args_with_build_flags",
)
load("@rules_cc//cc:cc_binary.bzl", "cc_binary")
load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("//build/bazel/rules/host_tests:host_test.bzl", "host_test")

visibility("private")

def build_flags_cc_binary_host_test(
        name,
        build_flags = [],
        disable_build_flags = [],
        **kwargs):
    """Define a new C++ or C host test for build_flags() support.

    Args:
       name: (string) Name of the host_test() target, the binary will be named "{name}_bin".
       build_flags: (list[label]) optional list of labels to build_flags() targets.
       disable_build_flags: (list[label]) optional list of labels to build_flags() targets.
       **kwargs: All other arguments are passed to cc_binary().
    """
    binary_name = "{}_bin".format(name)

    new_kwargs = wrap_cc_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        cc_rule_name = "cc_binary",
        build_flags = build_flags,
        disable_build_flags = disable_build_flags,
        target_type = "cxx_executable",
    )

    cc_binary(
        name = binary_name,
        **new_kwargs
    )

    host_test(
        name = name,
        binary = binary_name,
    )

def build_flags_cc_library(
        name,
        build_flags = [],
        disable_build_flags = [],
        **kwargs):
    """Define a new C++ library for build_flags() support.

    Args:
       name: (string) Name of the library target.
       build_flags: (list[label]) optional list of labels to build_flags() targets.
       disable_build_flags: (list[label]) optional list of labels to build_flags() targets.
       **kwargs: All other arguments are passed to cc_library().
    """
    new_kwargs = wrap_cc_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        cc_rule_name = "cc_library",
        build_flags = build_flags,
        disable_build_flags = disable_build_flags,
        target_type = "cxx_common",
    )

    cc_library(
        name = name,
        **new_kwargs
    )
