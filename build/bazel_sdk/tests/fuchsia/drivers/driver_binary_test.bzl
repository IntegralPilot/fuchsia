# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A test which verifies that driver binaries are built correctly. """

load(
    "@rules_fuchsia//fuchsia/private:fuchsia_transition.bzl",
    "fuchsia_transition",
)
load(
    "@rules_fuchsia//fuchsia_rules_common/debug_symbols:debug_symbols.bzl",
    "FUCHSIA_DEBUG_SYMBOLS_ATTRS",
    "strip_resources",
)
load("//test_utils:py_test_utils.bzl", "PY_TOOLCHAIN_DEPS", "create_python3_shell_wrapper_provider")

def _driver_binary_test_impl(ctx):
    # We normally strip the binary when we do our packaging but we don't need to
    # create an entire Fuchsia package to run this test so we just strip it here.
    resource = struct(
        src = ctx.files.driver[0],
        dest = "__not_needed__",
    )
    stripped_resources, _debug_info = strip_resources(ctx, [resource])
    driver = stripped_resources[0].src

    py_script_path = ctx.executable._binary_checker.short_path
    script_args = [
        "--driver_binary={}".format(driver.short_path),
        "--readelf={}".format(ctx.executable._readelf.short_path),
    ]
    script_runfiles = ctx.runfiles(
        files = [
            ctx.executable._readelf,
            driver,
        ],
    ).merge(ctx.attr._binary_checker[DefaultInfo].default_runfiles)

    return create_python3_shell_wrapper_provider(
        ctx,
        py_script_path,
        script_args,
        script_runfiles,
    )

driver_binary_test = rule(
    doc = """Validate the driver binary.""",
    test = True,
    implementation = _driver_binary_test_impl,
    toolchains = ["@bazel_tools//tools/cpp:toolchain_type"],

    # As noted in the implementation, we are not creating a Fuchsia package, so
    # we need to specify the transition ourselves.
    cfg = fuchsia_transition,
    attrs = {
        "driver": attr.label(
            doc = "The driver binary.",
            mandatory = True,
        ),
        "_binary_checker": attr.label(
            default = ":verify_driver_binary",
            executable = True,
            cfg = "exec",
        ),
        "fuchsia_api_level": attr.string(
            doc = "The fuchsia api level to target for this test.",
        ),
        "_readelf": attr.label(
            default = "@fuchsia_clang//:bin/llvm-readelf",
            executable = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    } | PY_TOOLCHAIN_DEPS | FUCHSIA_DEBUG_SYMBOLS_ATTRS,
)
