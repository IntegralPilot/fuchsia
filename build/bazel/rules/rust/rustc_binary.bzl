# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A macro for defining a Rust binary with optional unit tests."""

load(
    "@fuchsia_rules_common//build_flags:rust.bzl",
    "BUILD_FLAGS_RUST_ATTRS_KWARGS",
    "wrap_rust_macro_args_with_build_flags",
)
load("@rules_rust//rust:defs.bzl", "rust_binary")
load("//build/bazel/rules/rust:common.bzl", "with_fuchsia_rustc_flags")
load("//build/bazel/rules/rust:generate_unit_tests.bzl", "generate_unit_tests")

def _rustc_binary_impl(
        name,
        with_host_unit_tests,
        with_unit_tests,
        test_deps,
        lint_config,
        rustc_flags,
        build_flags,
        visibility,
        **kwargs):
    if lint_config == None:
        lint_config = "//build/config/rust/lints:clippy_warn_production"
        test_lint_config = "//build/config/rust/lints:clippy_warn_default"
    else:
        test_lint_config = lint_config

    kwargs["rustc_flags"] = with_fuchsia_rustc_flags(rustc_flags)

    binary_kwargs = wrap_rust_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        rust_rule_name = "rust_binary",
        build_flags = build_flags,
        target_type = "rust_executable",
    )

    rust_binary(
        name = name,
        lint_config = lint_config,
        visibility = visibility,
        **binary_kwargs
    )

    if with_host_unit_tests or with_unit_tests:
        test_kwargs = dict(binary_kwargs)

        # The following values are set for rust_binary() but are not supported by rust_test()
        # so remove them from kwargs to avoid Bazel error messages.
        test_kwargs.pop("output_licenses", None)
        test_kwargs.pop("crate_type", None)
        test_kwargs.pop("linker_script", None)
        test_kwargs.pop("binary_name", None)
        test_kwargs.pop("out_binary", None)

        generate_unit_tests(
            name = name,
            with_host_unit_tests = with_host_unit_tests,
            with_unit_tests = with_unit_tests,
            test_deps = test_deps,
            lint_config = test_lint_config,
            visibility = visibility,
            **test_kwargs
        )

rustc_binary = macro(
    doc = """`rust_binary` wrapper with Fuchsia-specific features.

Applies Fuchsia-specific Rust flags.

Generate a test target when either of with_host_unit_tests or
with_unit_tests is enabled. The test target will be named "<name>_test"
and will include extra dependencies from test_deps.

The default lint_config value is //build/config/rust/lints:clippy_warn_production
for the target, and //build/config/rust/lints:clippy_warn_default for the test target.
If specified by the caller, lint_config is applied to both main and test targets.

Also used to allow easier syncing between Bazel and GN targets.
See details in https://fxbug.dev/407441714.
""",
    implementation = _rustc_binary_impl,
    inherit_attrs = rust_binary,
    attrs = {
        "with_host_unit_tests": attr.bool(
            doc = "If true, a `host_rustc_test` target will be created. Incompatible with with_unit_tests.",
            default = False,
            configurable = False,
        ),
        "with_unit_tests": attr.bool(
            doc = "If true, a `rust_test` target will be created. Incompatible with with_host_unit_tests.",
            default = False,
            configurable = False,
        ),
        "test_deps": attr.label_list(
            doc = "Extra dependencies for the test target.",
            default = [],
        ),
    } | BUILD_FLAGS_RUST_ATTRS_KWARGS,
)
