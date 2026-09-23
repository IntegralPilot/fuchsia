# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Task for running a shell command."""

load(":fuchsia_task.bzl", "fuchsia_task_rule")

def shell_task_rule(*, implementation, attrs = {}, **kwargs):
    def _shell_task_impl(ctx, make_fuchsia_task):
        def _make_shell_task(command = [], runfiles = [], **kwargs):
            executable, arguments = (command[0], command[1:]) if command else (None, [])
            if type(executable) == "Target":
                runfiles.append(executable)
                command = [
                    executable[DefaultInfo].files_to_run.executable,
                ] + arguments

            return make_fuchsia_task(ctx.attr._shell_task_runner, command, runfiles = runfiles, **kwargs)

        return implementation(ctx, _make_shell_task)

    return fuchsia_task_rule(
        implementation = _shell_task_impl,
        attrs = dict(attrs, **{
            # TODO(chandarren): Support regex capture-and-export stdout as workflow state.
            # "capture_state": attr.string_dict(
            #     doc = "Export environment variables (name keys) as matched captured stdout (regex values).",
            # ),
            "_shell_task_runner": attr.label(
                doc = "The task runner used to run shell tasks.",
                default = "//fuchsia/tools:fuchsia_shell_task",
                executable = True,
                cfg = "exec",
            ),
        }),
        **kwargs
    )

def _fuchsia_shell_task_impl(ctx, make_shell_task):
    return make_shell_task([ctx.attr.executable] if ctx.attr.executable else [])

# buildifier: disable=unused-variable
__fuchsia_shell_task, _fuchsia_shell_task_for_test, _fuchsia_shell_task = shell_task_rule(
    doc = """Task for running a shell command.""",
    implementation = _fuchsia_shell_task_impl,
    attrs = {
        "executable": attr.label(
            doc = "Specify a bazel target as the shell executable.",
        ),
    },
)

def fuchsia_shell_task(
        *,
        command = None,
        target = None,
        arguments = [],
        **kwargs):
    """Creates a shell task.

    Args:
        command: The command to execute. May be a string or list of strings.
            Mutually exclusive with `target` and `arguments`.
        target: Optionally specify a bazel target as the executable.
            Mutually exclusive with `command`.
        arguments: A list of command line arguments to pass to `command`.
            Not allowed if `command` is a list.
        **kwargs: Additional arguments to forward to the base task rule.
    """
    if bool(command) == bool(target):
        fail("Argument `command` is mutually exclusive with argument `target`.")
    if type(command) == "list" and arguments:
        fail("Please append any arguments to `command`.")

    _fuchsia_shell_task(
        arguments = (command or []) + arguments,
        executable = target,
        **kwargs
    )
