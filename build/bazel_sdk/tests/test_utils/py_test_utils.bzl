# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Python test utilities."""

load("@bazel_skylib//lib:shell.bzl", "shell")

# Test rule definition whose actions invoke a python binary must do the following:
#
# - Merge the PY_TOOLCHAIN_DEPS dictionary to their `attr` argument to add
#   required dependencies.
#
# - Call either create_python3_shell_wrapper_provider() to return
#   a DefaultInfo value corresponding to the generated shell script
#   and its runtime requirements.
#
# This ensures that the interpreter and all dependent files are available during
# test execution. For example:
#
#   def _my_python_test_impl(ctx):
#       return [
#           create_python3_shell_wrapper_provider(
#               ctx,
#               ctx.attr.script.short_path,
#               ctx.attr.script_args,
#               ctx.runfiles(files = ctx.files.data),
#           ),
#       ]
#
#   my_python_test = rule(
#       implementation = _my_rule_impl,
#       test = True,
#       attrs = {
#           "script": attr.label(
#               mandatory = True,
#               allow_single_file = True,
#               doc = "Label to Python3 script to invoke at runtime for this test.",
#           ),
#           "script_args": attr.string_list(
#               default = [],
#               doc = "List of extra arguments to pass to the script.",
#           ),
#           "data": attr.label_list(
#               doc = "List of extra runtime dependencies for the script."
#           ),
#       } | PY_TOOLCHAIN_DEPS,
#   )
#

PY_TOOLCHAIN_DEPS = {
    "_py_toolchain": attr.label(
        default = "@rules_python//python:current_py_toolchain",
        cfg = "exec",
        providers = [DefaultInfo, platform_common.ToolchainInfo],
    ),
}

# Script template
_python3_shell_script_template = """\
#!/bin/sh
exec {py3_exec} -S {py3_script} {args} "$@"
"""

def create_python3_shell_wrapper_provider(ctx, py3_script_path, args = [], runfiles = None):
    """Create a shell script that invokes a given Python script for tests.

    This function can be called from a rule implementation function, and
    produces an output File instance (for the generated shell script)
    and a runfiles value (for its runtime dependencies, which include
    any required Python toolchain runtime files).

    The rule definition must use PY_TOOLCHAIN_DEPS.

    usage example:

        def _my_python_test_impl(ctx):
            return [
                create_python3_shell_wrapper_provider(
                    ctx,
                    ctx.attr.script.short_path,
                    ctx.attr.script_args,
                    ctx.runfiles(files = ctx.files.data),
                ),
            ]

        my_python_test = rule(
            implementation = _my_rule_impl,
            test = True,
            attrs = {
                "script": attr.label(
                    mandatory = True,
                    allow_single_file = True,
                    doc = "Label to Python3 script to invoke at runtime for this test.",
                ),
                "script_args": attr.string_list(
                    default = [],
                    doc = "List of extra arguments to pass to the script.",
                ),
                "data": attr.label_list(
                    doc = "List of extra runtime dependencies for the script."
                ),
            } | PY_TOOLCHAIN_DEPS,
        )

    Args:
        ctx: Rule context
        py3_script_path: Short path to python script.
        args: [string list] optional list of extra arguments.
        runfiles: [runfiles] Optional runfiles value for files required at runtime.
    Returns:
        A DefaultInfo provider for the generated script and its runtime requirements.
    """
    if not hasattr(ctx.attr, "_py_toolchain"):
        fail("The rule calling this function must use PY_TOOLCHAIN_DEPS!")

    toolchain_info = ctx.attr._py_toolchain[platform_common.ToolchainInfo]
    if not toolchain_info.py3_runtime:
        fail("A Bazel python3 runtime is required, and none was configured!")

    python3_executable = toolchain_info.py3_runtime.interpreter
    python3_runfiles = ctx.runfiles(transitive_files = toolchain_info.py3_runtime.files)

    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    script_content = _python3_shell_script_template.format(
        py3_exec = python3_executable.short_path,
        py3_script = py3_script_path,
        args = " ".join([shell.quote(a) for a in args]),
    )
    ctx.actions.write(script, script_content, is_executable = True)

    if runfiles == None:
        runfiles = python3_runfiles
    else:
        runfiles = runfiles.merge(python3_runfiles)

    return DefaultInfo(
        executable = script,
        runfiles = runfiles,
    )
