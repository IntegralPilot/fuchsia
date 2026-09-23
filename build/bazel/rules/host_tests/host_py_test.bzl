# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros for defining `host_test()` targets for Python binaries."""

load("@rules_python//python:py_binary.bzl", "py_binary")
load("@rules_python//python:py_info.bzl", "PyInfo")
load(":host_test.bzl", "host_test")

def _host_py_test_info_impl(ctx):
    binary_label = ctx.attr.binary.label
    binary_repo = binary_label.repo_name
    binary_package = binary_label.package

    # The main import path corresponds to the runfiles sub-directory that contains
    # the test binary. This will be "_main" for targets defined in the main workspace,
    # or <canonical_repo_name> for targets defined in external repositories.
    main_import = binary_repo if binary_repo else "_main"

    # Use the main import path, with the import paths from all transitive dependencies.
    imports = depset([main_import], transitive = [ctx.attr.binary[PyInfo].imports]).to_list()

    output = ctx.actions.declare_file(ctx.label.name)
    output_json = {
        # The main module is always in the same package, but may use a different name
        # than the binary label.
        "binary": "{}/{}/{}".format(main_import, binary_package, ctx.attr.main),
        "imports": imports,
    }

    ctx.actions.write(output, json.encode(output_json))

    return [DefaultInfo(files = depset([output]))]

_host_py_test_info = rule(
    implementation = _host_py_test_info_impl,
    doc = """Generate information about a Python host test used to generate its wrapper script.

    This writes a single JSON file containing an object with following schema:

    # LINT.IfChange(bazel_host_py_test_info_schema)

      "binary": runfiles path of the test's py_binary module. E.g. "_main/src/foo/test.py".
      "imports": List of import paths for the test's py_binary, relative to the runfiles directory.

    # LINT.ThenChange(//scripts/fxtest/list_host_test/list_host_python_unittests.py:bazel_host_py_test_info_schema)
    """,
    attrs = {
        "binary": attr.label(mandatory = True, providers = [PyInfo]),
        "main": attr.string(mandatory = True),
    },
)

def legacy_host_py_test(
        name,
        binary_name = "",
        main = None,
        test_args = [],
        test_data = [],
        tags = [],
        visibility = None,
        **kwargs):
    """Define a host test wrapping a Python binary that can be used with Fuchsia test runners.

    This is a convenience macro to call py_binary() and host_test() together.

    Unlike py_test(), these tests will be usable with `fx test` and `botanist`, and can
    still be run locally using `fx bazel test --config=host <label>`.

    Args:
      name: The name of the host test.
      binary_name: Optional. The name of the py_binary target, defaults to 'name + "_bin"'.
      main: Optional. The main entry point for the py_binary, defaults to 'name + ".py"'.
      test_args: Arguments to pass to the test binary. Do not use `args`.
      test_data: Optional. The data dependencies for the test target itself.
      **kwargs: Arguments to pass to `py_binary`.
    """
    if "args" in kwargs:
        fail("Use `test_args` to pass test arguments instead of `args`")

    binary_name = binary_name if binary_name else name + "_bin"
    host_py_test_info = name + ".host_py_test_info"

    if "manual" not in tags:
        tags = tags + ["manual"]

    if not main:
        main = name + ".py"

    py_binary(
        name = binary_name,
        main = main,
        visibility = ["//visibility:private"],
        **kwargs
    )

    _host_py_test_info(
        name = host_py_test_info,
        binary = ":" + binary_name,
        main = main,
        visibility = ["//visibility:private"],
    )

    test_data = test_data + [
        # The list_host_test tool is used to implement `fx test --list <test_name>`.
        "//scripts/fxtest/list_host_test:as_test_data",

        # The python_interpreter is used by the wrapper script directly.
        "//build/bazel/rules/python:python_interpreter_as_test_data",
    ]

    host_test(
        name = name,
        binary = binary_name,
        test_args = test_args,
        data = test_data,
        list_cases_argument = "list_host_python_unittests",
        host_test_wrapper_template = "//build/bazel:templates/template.host_py_test_wrapper.sh",
        host_test_wrapper_generator_deps = [
            ":" + host_py_test_info,
        ],
        host_test_wrapper_generator_args = [
            "--python-test-lister=test_data/list_host_python_unittests.py",
            "--python-test-interpreter=test_data/python3",
            "--python-test-info=$(location :{})".format(host_py_test_info),
        ],
        visibility = visibility,
    )

def _host_py_test_impl(name, visibility, binary_name = "", main = None, test_args = [], test_data = [], tags = [], **kwargs):
    legacy_host_py_test(
        name = name,
        binary_name = binary_name,
        main = main,
        test_args = test_args,
        test_data = test_data,
        tags = tags,
        visibility = visibility,
        **kwargs
    )

# This is a broken attempt to create a symbolic macro that would inherit all attributes,
# unfortunately, it requires an inherit_attrs line that fails with both py_binary and
# native.py_binary. Hopefully this could be fixed with a future Bazel upgrade.
broken_symbolic_host_py_test = macro(
    implementation = _host_py_test_impl,
    doc = """
Define a host test wrapping a Python binary that can be used with Fuchsia test runners.

This is a convenience macro to call py_binary() and host_test() together.

Unlike py_test(), these tests will be usable with `fx test` and `botanist`, and can
still be run locally using `fx bazel test --config=host <label>`.

Accepts all py_binary() attributes, plus `binary_name` and `test_xxx` ones.
""",
    # The inherit_attrs line below fails with **both** py_binary and native.py_binary with
    # an error message like:
    # ```
    # Error in macro: in call to macro(), parameter 'inherit_attrs' got value of type 'function', want 'rule, macro, string, or NoneType'
    # ```
    # inherit_attrs = native.py_binary,
    attrs = {
        "binary_name": attr.string(default = "", doc = "The name of the py_binary target, defaults to 'name + \"_bin\"'."),
        "test_args": attr.string_list(default = [], doc = "Arguments to pass to the test binary. Do not use `args`."),
        "test_data": attr.label_list(default = [], doc = "Data dependencies for the test target itself."),
    },
)

# Switch to symbolic macro once the inherit_attrs error is fixed.
host_py_test = legacy_host_py_test
