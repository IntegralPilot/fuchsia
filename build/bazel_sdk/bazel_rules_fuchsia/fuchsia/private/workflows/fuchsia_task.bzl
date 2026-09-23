# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Base task rule and utilities."""

load(":providers.bzl", "FuchsiaTaskEntityInfo", "FuchsiaWorkflowInfo")
load(":workflow_entity.bzl", "workflow_entity_rule")

def fuchsia_task_rule(*, implementation, **kwargs):
    """Starlark higher-order rule for creating task primitives."""

    def _fuchsia_task_impl(ctx, make_workflow, collect_arguments):
        def _make_fuchsia_task(task_runner, prepend_args = [], runfiles = [], default_argument_scope = "explicit"):
            task_args, task_runfiles = collect_arguments(prepend_args, runfiles, task_runner)
            return make_workflow(
                FuchsiaWorkflowInfo(
                    entities = {
                        ctx.label: FuchsiaTaskEntityInfo(
                            task_runner = task_runner[DefaultInfo].files_to_run.executable.short_path,
                            args = task_args,
                            default_argument_scope = ctx.attr.default_argument_scope or default_argument_scope,
                        ),
                    },
                    entrypoint = ctx.label,
                ),
                task_runfiles,
            )

        return implementation(ctx, _make_fuchsia_task)

    return workflow_entity_rule(
        implementation = _fuchsia_task_impl,
        **kwargs
    )

def _fuchsia_task_impl(ctx, make_fuchsia_task):
    return make_fuchsia_task(ctx.attr.task_runner)

_fuchsia_task, _fuchsia_task_for_test, fuchsia_task = fuchsia_task_rule(
    implementation = _fuchsia_task_impl,
    doc = """Build-rule version of `fuchsia_task_rule`.""",
    attrs = {
        "task_runner": attr.label(
            mandatory = True,
            doc = "The task runner used to run this task.",
            executable = True,
            cfg = "exec",
        ),
    },
)
