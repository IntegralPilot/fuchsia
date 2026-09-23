# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helper rules for creating workflow entities."""

load(
    ":utils.bzl",
    "alias",
    "collect_runfiles",
    "rule_variants",
    "wrap_executable",
)

def workflow_entity_rule(*, implementation, attrs = {}, **kwargs):
    def workflow_entity_impl(ctx):
        if ctx.attr.args:
            fail("Please use the `arguments` attribute instead of `args`.")

        def _make_workflow(workflow, *runfiles):
            """Generates a workflow runner invocation given a FuchsiaWorkflowInfo and returns all providers.

            Args:
                workflow: The workflow to provide the workflow runner.
                *runfiles: Additional runfiles dependencies for this workflow entity.

            Returns:
                A DefaultInfo invocation of the corresponding workflow and the FuchsiaWorkflowInfo.
            """
            manifest = ctx.actions.declare_file("%s_workflow.json" % ctx.attr.name.removesuffix("_base"))
            ctx.actions.write(manifest, json.encode_indent({
                "entities": {
                    str(label).removesuffix("_base"): {
                        "type": "task",
                        "task_runner": entity.task_runner,
                        "args": entity.args,
                        "default_argument_scope": entity.default_argument_scope,
                    } if hasattr(entity, "task_runner") else {
                        "type": "workflow",
                        "sequence": [str(step).removesuffix("_base") for step in entity.sequence],
                        "args": entity.args,
                    }
                    for label, entity in workflow.entities.items()
                },
                "entrypoint": str(workflow.entrypoint).removesuffix("_base"),
            }))

            invocation, runner_runfiles = wrap_executable(
                ctx,
                ctx.attr._unbuffer_tool,
                ctx.attr._run_workflow,
                "--workflow-manifest",
                manifest,
                script_name = "%s.sh" % ctx.attr.name.removesuffix("_base"),
            )
            return [
                workflow,
                DefaultInfo(
                    executable = invocation,
                    files = depset([invocation, manifest]),
                    runfiles = collect_runfiles(
                        ctx,
                        runner_runfiles,
                        ctx.attr.inputs,
                        *runfiles
                    ),
                ),
            ]

        def _collect_arguments(prepend_task_args = [], *runfiles):
            """Interpolates task arguments and collects runfiles."""
            arguments = prepend_task_args + ctx.attr.arguments
            task_runfiles = collect_runfiles(ctx, ctx.attr.inputs, runfiles, arguments, ignore_types = ["string"])

            # TODO(https://fxbug.dev/42065778): Interpolate input file locations.
            return [
                arg.short_path if type(arg) == "File" else arg
                for arg in arguments
            ], task_runfiles

        return implementation(ctx, _make_workflow, _collect_arguments)

    rules = rule_variants(
        implementation = workflow_entity_impl,
        variants = ("executable", "test"),
        attrs = dict(attrs, **{
            "_unbuffer_tool": attr.label(
                default = "//fuchsia/tools:unbuffer",
                doc = "Pass-through executable that unbuffers command stdout/stderr.",
                executable = True,
                cfg = "exec",
            ),
            "_run_workflow": attr.label(
                default = "//fuchsia/tools:run_workflow",
                doc = "The workflow runner tool.",
                executable = True,
                cfg = "exec",
            ),
            "arguments": attr.string_list(
                doc = "Specify arguments for this workflow entity.",
            ),
            "default_argument_scope": attr.string(
                doc = "The scope of arguments to use for the workflow entity.",
                default = "",
                values = ["", "explicit", "workflow", "global"],
            ),
            "inputs": attr.label_list(
                doc = "Task dependencies. Use `$location(path/to/file)` to reference these in arguments.",
            ),
        }),
        **kwargs
    )

    def macro(
            *,
            name,
            testonly = False,
            tags = [],
            visibility = None,
            applicable_licenses = None,
            **kwargs):
        # Switch between the test and non-test workflow variant based on testonly.
        rules[1 if testonly else 0](
            name = name + "_base",
            tags = tags + ["manual"],
            visibility = visibility,
            applicable_licenses = applicable_licenses,
            **kwargs
        )

        alias(
            name = name,
            actual = name + "_base",
            executable = True,
            testonly = testonly,
            applicable_licenses = applicable_licenses,
            tags = tags,
            visibility = visibility,
        )

    # Because cc_test needs all transitive dependent ancestors to be test rules
    # (testonly is not sufficient), we return 3 values to be unpacked:
    # 1. The non-test workflow entity rule variant,
    # 2. The test workflow entity rule variant, and
    # 3. A wrapper macro that invokes either `1` or `2` based on `testonly`.
    #    The wrapper macro can also optionally apply a fuchsia_transition.
    #
    # We can't omit `1` or `2`, since bazel needs each rule to be assigned a
    # name and exportable by a `.bzl` file.
    return rules + [macro]
