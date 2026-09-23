# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Expresses an execution order for tasks."""

load(":providers.bzl", "FuchsiaWorkflowEntityInfo", "FuchsiaWorkflowInfo")
load(":workflow_entity.bzl", "workflow_entity_rule")

def _collect_entities(initial, steps):
    # Basically merge N dictionaries.
    for step_entities in steps:
        for k, v in step_entities.items():
            if k in initial:
                # Sanity check.
                if (
                    type(initial[k]) != type(v)
                ) or (
                    initial[k].task_runner != v.task_runner if (
                        hasattr(v, "task_runner")
                    ) else initial[k].sequence != v.sequence
                ) or (
                    initial[k].args != v.args
                ):
                    fail("Invalid workflow state.")
            else:
                initial[k] = v
    return initial

def fuchsia_workflow_rule(*, implementation, **kwargs):
    """Starlark higher-order rule for specifying a sequence of workflow entities."""

    def _fuchsia_workflow_impl(ctx, make_workflow_entity, collect_arguments):
        def _make_workflow(sequence, prepend_args = [], runfiles = []):
            workflow_args, runfiles = collect_arguments(prepend_args, runfiles)
            return make_workflow_entity(
                FuchsiaWorkflowInfo(
                    entities = _collect_entities({
                        ctx.label: FuchsiaWorkflowEntityInfo(
                            sequence = [dep[FuchsiaWorkflowInfo].entrypoint for dep in sequence],
                            args = workflow_args,
                        ),
                    }, [dep[FuchsiaWorkflowInfo].entities for dep in sequence]),
                    entrypoint = ctx.label,
                ),
                sequence,
                runfiles,
            )

        return implementation(ctx, _make_workflow)

    return workflow_entity_rule(
        implementation = _fuchsia_workflow_impl,
        **kwargs
    )

def _fuchsia_workflow_impl(ctx, make_workflow):
    return make_workflow(ctx.attr.sequence)

_fuchsia_workflow, _fuchsia_workflow_for_test, fuchsia_workflow = fuchsia_workflow_rule(
    implementation = _fuchsia_workflow_impl,
    doc = """A grouping of tasks to be run sequentially.""",
    attrs = {
        "sequence": attr.label_list(
            doc = "The order of tasks to run.",
            providers = [FuchsiaWorkflowInfo],
        ),
    },
)
