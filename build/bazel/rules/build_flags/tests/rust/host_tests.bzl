# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//build_flags:rust.bzl",
    "wrap_rust_macro_args_with_build_flags",
)
load("@rules_rust//rust:rust_binary.bzl", "rust_binary")
load("//build/bazel/rules/host_tests:host_test.bzl", "host_test")

visibility("private")

def build_flags_rust_binary_host_test(
        name,
        build_flags = [],
        **kwargs):
    """Define a new Rust host test for build_flags() support.

    This creates a rust_binary() target and associated host_test() wrapper.
    This checks the implementation of wrap_rust_rule_with_build_flags()
    in isolation, independent of modification to the rustc_binary()
    wrapper macro.

    Args:
       name: (string) Name of the host_test() target, the binary will be named "{name}_bin".
       build_flags: (list[label]) optional list of labels to build_flags() targets.
       **kwargs: All other arguments are passed to rustc_binary().
    """
    binary_name = "{}_bin".format(name)

    new_kwargs = wrap_rust_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        rust_rule_name = "rust_binary",
        build_flags = build_flags,
        target_type = "rust_executable",
    )

    rust_binary(
        name = binary_name,
        **new_kwargs
    )

    host_test(
        name = name,
        binary = binary_name,
    )
