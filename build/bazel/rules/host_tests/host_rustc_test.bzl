# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros for defining `host_test()` targets for Rust binaries."""

load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("//build/bazel/rules/rust:rustc_test.bzl", "rustc_test")
load(":host_test.bzl", "host_test")
load(":host_test_data.bzl", "host_test_data_files")

def wrap_host_rust_test(
        *,
        name,
        binary_name,
        test_label = None,
        test_args = [],
        test_data = [],
        visibility = None):
    """Generate a host_test() target to run a rust_test().

    This ensures that a host rust_test() binary can be run with 'fx test' and in infra builders
    by creating a wrapping `host_test()` target with the right set of dependencies.

    Note that test arguments must be passed here in `test_args`, as Bazel does not record
    the `rust_test()`'s own `args` values in providers, making them unavailable to the
    wrapper target.

    Args:
       name: (string) host_test() target name
       binary_name: (string) label to rust_test() binary.
       test_label: (Optional[string]) Optional test_label to pass to host_test(). Default to None.
       test_args: (list[string]) List of test arguments.
       test_data: (list[string]) List of labels to test's runtime requirements.
       visibility: (list[string]) Visibility of the final host_test() target.
    """
    binary_as_test_data = binary_name + "_as_test_data"
    host_test_data_files(
        name = binary_as_test_data,
        srcs = [":" + binary_name],
        testonly = True,
        visibility = ["//visibility:private"],
    )

    # Wrap the binary invocation with //tools/rust_test_parser
    # LINT.IfChange(rustc_test_invocation)
    wrapper_script = "//tools/rust_test_parser:rust_test_parser"

    # In Bazel, and unlike GN, rustc_test() binaries always contain debug
    # symbols (see //build/bazel/debug_symbols/README.md for details).
    #
    # Enable Rust stack traces by default. In GN, when an ASan or Coverage
    # toolchain is detected, this is disabled and the test uses the stripped
    # binary instead. This is not implemented yet  because there is no good
    # way to track these in Bazel at the moment.
    #
    # TODO(https://fxbug.dev/495755822): Disable for ASan and Coverage build
    # configurations.
    test_args = [
        "env RUST_BACKTRACE=1",
        "./{}".format(binary_name),
    ] + test_args

    test_data = test_data + [":" + binary_as_test_data]

    # LINT.ThenChange(//build/rust/rustc_test.gni:rustc_test_invocation)

    host_test(
        name = name,
        binary = wrapper_script,
        unstripped_binary = ":" + binary_name,
        test_label = test_label,
        test_args = test_args,
        data = test_data,
        target_compatible_with = HOST_CONSTRAINTS,
        visibility = visibility,
    )

def define_and_wrap_host_rust_test(
        *,
        name,
        rust_test_rule,
        binary_name = None,
        test_label = None,
        test_args = [],
        test_data = [],
        tags = [],
        visibility,
        **kwargs):
    """Generate a host_test() target wrapping a new rust_test() target.

    This is a convenience macro to call rust_test_rule() and wrap_host_rust_test()
    together. Unlike rust_test() targets, these tests will be usable with
    `fx test` and `botanist`, and can still be run locally using
    `fx bazel test --config=host <label>`.

    Note that test arguments must be passed here in `test_args`, as Bazel does not record
    the `rust_test()`'s own `args` values in providers, making them unavailable to the
    wrapper target.

    The "manual" tag will be set on the leaf rust_test() target. Hence something
    like `fx bazel test --config=host //build/bazel/rules/host_tests/tests/rust/...`
    will correctly only run one test target, instead of two.

    Args:
      name: (string) The name of the parent host_test() target.
      rust_test_rule: (rule) The rule used to define the real rust_test() target. This
         can be either rust_test() or rustc_test(), the latter implementing
         Fuchsia-specific features.
      binary_name: (string) Optional. The name of the rust_test_rule() target, defaults to
          "<name>_bin".
      test_label: (string) Optional test label to appear in tests.json.
      test_args: (string list) Optional arguments to pass to the test binary. Do not use `args`.
      test_data: (string list) Optional. The data dependencies for the test target itself.
      tags: (string list) Optional. List of test tags.
      visibility: (string list) Optional. Visibility of the top-level test target, the leaf
        rust_test() target will have private visibility instead.
      **kwargs: Arguments to pass to `rust_test_rule`.
    """
    if kwargs.get("args"):
        fail("Use `test_args` to pass test arguments instead of `args`")

    binary_name = binary_name if binary_name else name + "_bin"

    # When tags comes from expanding the kwargs of an inherited parent rule,
    # attributes such as tags can be set to None instead of an empty list. In
    # that case, default to an empty list.
    tags = (tags or [])
    if "manual" not in tags:
        tags = tags + ["manual"]

    rust_test_rule(
        name = binary_name,
        tags = tags,
        visibility = ["//visibility:private"],
        **kwargs
    )

    wrap_host_rust_test(
        name = name,
        test_label = test_label,
        binary_name = binary_name,
        test_args = test_args,
        test_data = test_data,
        visibility = visibility,
    )

def legacy_host_rustc_test(
        name,
        binary_name = "",
        test_label = None,
        test_args = [],
        test_data = [],
        tags = [],
        visibility = None,
        **kwargs):
    """Define a host test wrapping a Rust test that can be used with Fuchsia test runners.

    This is a convenience macro to call rustc_test() and host_test() together.

    Unlike rustc_test(), these tests will be usable with `fx test` and `botanist`, and can
    still be run locally using `fx bazel test --config=host <label>`.

    The "manual" tag will be set on the rustc_test() target. In practice something like
    `fx bazel test --config=host //build/bazel/rules/host_tests/tests/rust/...`
    will correctly only run one test target, instead of two for each host_rustc_test()
    definition.

    Args:
      name: The name of the host test.
      binary_name: Optional. The name of the rustc_test target, defaults to 'name + "_bin"'.
      test_args: Arguments to pass to the test binary. Do not use `args`.
      test_data: Optional. The data dependencies for the test target itself.
      tags: Optional: List of test tags.
      **kwargs: Arguments to pass to `rustc_test`.
    """
    define_and_wrap_host_rust_test(
        name = name,
        rust_test_rule = rustc_test,
        test_label = test_label,
        test_args = test_args,
        test_data = test_data,
        tags = tags,
        visibility = visibility,
        **kwargs
    )

def _host_rustc_test_impl(
        name,
        visibility,
        binary_name,
        test_label,
        test_args,
        test_data,
        tags,
        **kwargs):
    legacy_host_rustc_test(
        name = name,
        binary_name = binary_name,
        test_label = test_label,
        test_args = test_args,
        test_data = test_data,
        tags = tags,
        visibility = visibility,
        **kwargs
    )

# TODO(https://fxbug.dev/349341932): Switch to symbolic macro once the inherit_attrs error is fixed.
#
# This is a broken attempt to create a symbolic macro that would inherit all attributes.
#
# Unfortunately, it requires an inherit_attrs line that fails with rustc_test.
# Hopefully this could be fixed with a future Bazel upgrade.
broken_symbolic_host_rustc_test = macro(
    implementation = _host_rustc_test_impl,
    doc = """
Define a host test wrapping a Rust test that can be used with Fuchsia test runners.

This is a convenience macro to call rustc_test() and host_test() together.

Unlike rustc_test(), these tests will be usable with `fx test` and `botanist`, and can
still be run locally using `fx bazel test --config=host <label>`.

The "manual" tag will be set on the rustc_test() target. In practice something like
`fx bazel test --config=host //build/bazel/rules/host_tests/tests/rust/...`
will correctly only run one test target, instead of two for each host_rustc_test()
definition.

Accepts all rustc_test() attributes, plus `binary_name` and `test_xxx` ones.
""",
    inherit_attrs = rustc_test,
    attrs = {
        "binary_name": attr.string(default = "", doc = "The name of the rustc_test target, defaults to 'name + \"_bin\"'."),
        "test_label": attr.label(default = None, doc = "Optional override for the test_label passed to host_test()."),
        "test_args": attr.string_list(default = [], doc = "Arguments to pass to the test binary. Do not use `args`."),
        "test_data": attr.label_list(default = [], doc = "Data dependencies for the test target itself."),
    },
)

# Switch to symbolic macro once the inherit_attrs error is fixed.
host_rustc_test = legacy_host_rustc_test
