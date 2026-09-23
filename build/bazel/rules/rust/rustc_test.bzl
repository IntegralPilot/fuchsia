# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A macro for defining a Rust test with Fuchsia-specific lint config by default."""

load("@fuchsia_rules_common//build_flags:rust.bzl", "BUILD_FLAGS_RUST_ATTRS_KWARGS", "wrap_rust_macro_args_with_build_flags")
load("@rules_rust//rust:defs.bzl", "rust_test")
load("//build/bazel/rules/rust:common.bzl", "with_fuchsia_rustc_flags")

def _rustc_test_impl(
        name,
        lint_config,
        rustc_flags,
        build_flags,
        **kwargs):
    if lint_config == None:
        lint_config = "//build/config/rust/lints:clippy_warn_default"

    kwargs["rustc_flags"] = with_fuchsia_rustc_flags(rustc_flags)

    wrapped_kwargs = wrap_rust_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        rust_rule_name = "rust_test",
        build_flags = build_flags,
        target_type = "rust_executable",
    )

    rust_test(
        name = name,
        lint_config = lint_config,
        **wrapped_kwargs
    )

rustc_test = macro(
    doc = """Define a rust_test() target with Fuchsia-specific features

    Applies Fuchsia-specific Rust flags.

    lint_config is set to "//build/config/rust/lints:clippy_warn_default",
    unless specified by the user.

    IMPORTANT: The resulting Bazel test target is *not* visible to Fuchsia test
    runners. It must be exposed via a secondary mechanism. For example using
    wrap_host_rust_test() for host build configurations, or using it in a test
    component for Fuchsia ones.
    """,
    implementation = _rustc_test_impl,
    inherit_attrs = rust_test,
    attrs = BUILD_FLAGS_RUST_ATTRS_KWARGS,
)
