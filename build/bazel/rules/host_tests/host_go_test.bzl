# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros for defining `host_test()` targets for Go binaries."""

load("@io_bazel_rules_go//go:def.bzl", "go_test")
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load(":host_test.bzl", "host_test")
load(":host_test_data.bzl", "host_test_data_files")

def legacy_host_go_test(
        name,
        binary_name = "",
        test_args = [],
        test_data = [],
        tags = [],
        timeout = "5m",
        visibility = None,
        **kwargs):
    """Define a host test wrapping a Go binary that can be used with Fuchsia test runners.

    This is a convenience macro to call go_test() and host_test() together.

    Unlike go_test(), these tests will be usable with `fx test` and `botanist`, and can
    still be run locally using `fx bazel test --config=host <label>`.

    The "manual" tag will be set on the go_test() target. In practice something like
    `fx bazel test --config=host //build/bazel/rules/host_tests/tests/go/...`
    will correctly only run one test target, instead of two for each host_go_test()
    definition.

    Args:
      name: The name of the host test.
      binary_name: Optional. The name of the go_test target, defaults to 'name + "_bin"'.
      test_args: Arguments to pass to the test binary. Do not use `args`.
      test_data: Optional. The data dependencies for the test target itself.
      timeout: Optional. Override default timeout. Values must be valid Go durations
         such as "300ms", "1.5h" or "2h45m". See
         https://golang.org/cmd/go/#hdr-Testing_flags for details on timeout. See
         https://golang.org/pkg/time/#ParseDuration for duration format.
      **kwargs: Arguments to pass to `go_test`.
    """
    if "args" in kwargs:
        fail("Use `test_args` to pass test arguments instead of `args`")
    binary_name = binary_name if binary_name else name + "_bin"

    binary_as_test_data = binary_name + ".test_data"

    if "manual" not in tags:
        tags = tags + ["manual"]

    go_test(
        name = binary_name,
        tags = tags,
        visibility = ["//visibility:private"],
        **kwargs
    )

    host_test_data_files(
        name = binary_as_test_data,
        srcs = [":" + binary_name],
        testonly = True,
    )

    # Wrap the binary invocation with //tools/go_test_parser.
    test_data = test_data + [":" + binary_as_test_data]

    # LINT.IfChange(go_test_wrapper)
    wrapper_script = "//tools/go_test_parser:go_test_parser"
    test_args = [
        "./" + binary_name,
        "-test.timeout",
        timeout,
        "-test.v",  # Emit detailed test case information.
    ] + test_args
    # LINT.ThenChange(//build/go/go_test.gni:go_test_wrapper)

    host_test(
        name = name,
        binary = wrapper_script,
        test_args = test_args,
        data = test_data,
        target_compatible_with = HOST_CONSTRAINTS,
        visibility = visibility,
    )

def _host_go_test_impl(
        name,
        visibility,
        binary_name = "",
        test_args = [],
        test_data = [],
        tags = [],
        timeout = "5m",
        **kwargs):
    legacy_host_go_test(
        name = name,
        binary_name = binary_name,
        test_args = test_args,
        test_data = test_data,
        tags = tags,
        timeout = timeout,
        visibility = visibility,
        **kwargs
    )

# TODO(https://fxbug.dev/349341932): Switch to symbolic macro once the inherit_attrs error is fixed.
#
# This is a broken attempt to create a symbolic macro that would inherit all attributes.
#
# Unfortunately, it requires an inherit_attrs line that fails with go_test.
# Hopefully this could be fixed with a future Bazel upgrade.
broken_symbolic_host_go_test = macro(
    implementation = _host_go_test_impl,
    doc = """
Define a host test wrapping a Go binary that can be used with Fuchsia test runners.

This is a convenience macro to call go_test() and host_test() together.

Unlike go_test(), these tests will be usable with `fx test` and `botanist`, and can
still be run locally using `fx bazel test --config=host <label>`.

The "manual" tag will be set on the go_test() target. In practice something like
`fx bazel test --config=host //build/bazel/rules/host_tests/tests/go/...`
will correctly only run one test target, instead of two for each host_go_test()
definition.

Accepts all go_test() attributes, plus `binary_name`, `timeout` and `test_xxx` ones.
""",
    inherit_attrs = go_test,
    attrs = {
        "binary_name": attr.string(default = "", doc = "The name of the go_test target, defaults to 'name + \"_bin\"'."),
        "test_args": attr.string_list(default = [], doc = "Arguments to pass to the test binary. Do not use `args`."),
        "test_data": attr.label_list(default = [], doc = "Data dependencies for the test target itself."),
        "timeout": attr.string(default = "5m", doc = """
             Override default timeout. Values must be valid Go durations.
             such as "300ms", "1.5h" or "2h45m". See
             https://golang.org/cmd/go/#hdr-Testing_flags for details on timeout. See
             https://golang.org/pkg/time/#ParseDuration for duration format."""),
    },
)

# Switch to symbolic macro once the inherit_attrs error is fixed.
host_go_test = legacy_host_go_test
