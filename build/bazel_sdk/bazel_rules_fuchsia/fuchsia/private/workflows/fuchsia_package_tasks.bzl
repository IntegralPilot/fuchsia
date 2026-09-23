# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load(
    "@fuchsia_rules_common//debug_symbols:providers.bzl",
    "FuchsiaDebugSymbolInfo",
)
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load(":fuchsia_shell_task.bzl", "shell_task_rule")
load(":fuchsia_task_ffx.bzl", "fuchsia_task_ffx")
load(":fuchsia_task_publish.bzl", "fuchsia_task_publish")
load(":fuchsia_task_register_debug_symbols.bzl", "fuchsia_task_register_debug_symbols")
load(":fuchsia_task_run_component.bzl", "fuchsia_task_run_component")
load(":fuchsia_task_run_driver_tool.bzl", "fuchsia_task_run_driver_tool")
load(":fuchsia_task_test_enumerated_components.bzl", "fuchsia_task_test_enumerated_components")
load(":fuchsia_task_verbs.bzl", "make_help_executable", "verbs")
load(":fuchsia_workflow.bzl", "fuchsia_workflow", "fuchsia_workflow_rule")
load(":providers.bzl", "FuchsiaWorkflowInfo")
load(":utils.bzl", "flatten", "normalized_target_name")

def _to_verb(label):
    return verbs.custom(Label(label).name)

def _fuchsia_package_help_impl(ctx, make_shell_task):
    components = ctx.attr.package[FuchsiaPackageInfo].packaged_components
    help = make_help_executable(ctx, dict((
        [(verbs.noverb, "Run all test components within this test package.")] if ctx.attr.is_test and len(components) > 0 else []
    ) + (
        [(verbs.test_enumerated, "Enumerates and tests all components within this test package.")] if ctx.attr.enumerated_testing_workflow else []
    ) + [
        (verbs.help, "Print this help message."),
        (verbs.debug_symbols, "Register this package's debug symbols."),
        (verbs.publish, "Publish this package and register debug symbols."),
    ] + [
        (verbs.custom(component.component_info.run_tag), "Publish this package and run '%s' with debug symbols." % component.component_info.run_tag)
        for component in components
    ] + [
        (_to_verb(tool), "Publish this package and run '%s' with debug symbols" % tool)
        for tool in ctx.attr.tools
    ]), name = ctx.attr.top_level_name)
    return make_shell_task([help])

# buildifier: disable=unused-variable
(
    __fuchsia_package_help,
    _fuchsia_package_help_for_test,
    _fuchsia_package_help,
) = shell_task_rule(
    implementation = _fuchsia_package_help_impl,
    doc = "Prints valid runnable sub-targets in a package.",
    attrs = {
        "package": attr.label(
            doc = "The package.",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
        "is_test": attr.bool(
            doc = "Whether the package is a test package.",
            mandatory = True,
        ),
        "enumerated_testing_workflow": attr.bool(
            doc = "Whether the :pkg.test_enumerated target is generated.",
            mandatory = True,
        ),
        "tools": attr.string_list(
            doc = "The driver tool names.",
            mandatory = True,
        ),
        "debug_symbols_task": attr.label(
            doc = "The debug symbols task associated with the package.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "publish_task": attr.label(
            doc = "The package publishing task associated with the package.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "top_level_name": attr.string(
            doc = "The top level target name associated with these tasks",
            mandatory = True,
        ),
    },
)

def _fuchsia_package_default_task_impl(ctx, make_workflow):
    default_workflow = make_workflow(sequence = flatten([
        ctx.attr.debug_symbols_task,
        ctx.attr.publish_task,
    ] + ctx.attr.component_run_tasks + [
        ctx.attr.publish_cleanup_task or [],
    ]) if (
        ctx.attr.is_test and ctx.attr.component_run_tasks
    ) else [ctx.attr.help_task])
    return [
        DefaultInfo(
            files = depset(transitive = [provider.files, ctx.attr.package[DefaultInfo].files]),
            runfiles = provider.default_runfiles,
            executable = provider.files.to_list()[0],
        ) if type(provider) == "DefaultInfo" else provider
        for provider in default_workflow
    ] + [
        ctx.attr.package[FuchsiaPackageInfo],
        ctx.attr.package[FuchsiaDebugSymbolInfo],
        # Expose the generated far file and debug symbols.
        # This is also used in fuchsia.git, see https://fxbug.dev/42066998 and
        # https://fxbug.dev/42070079.
        OutputGroupInfo(
            far_file = depset([ctx.attr.package[FuchsiaPackageInfo].far_file]),
            build_id_dirs = depset(transitive = ctx.attr.package[FuchsiaDebugSymbolInfo].build_id_dirs_mapping.values()),
        ),
    ]

# buildifier: disable=unused-variable
(
    __fuchsia_package_default_task,
    _fuchsia_package_default_task_for_test,
    _fuchsia_package_default_task,
) = fuchsia_workflow_rule(
    implementation = _fuchsia_package_default_task_impl,
    doc = "Runs all test components for test packages, or prints a help message.",
    attrs = {
        "is_test": attr.bool(
            doc = "Whether the package is a test package.",
            mandatory = True,
        ),
        "help_task": attr.label(
            doc = "The help task describing valid package subtargets.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "debug_symbols_task": attr.label(
            doc = "The debug symbols task associated with the package.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "publish_task": attr.label(
            doc = "The package publishing task associated with the package.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "publish_cleanup_task": attr.label(
            doc = "The package publishing cleanup task associated with the package.",
            providers = [FuchsiaWorkflowInfo],
        ),
        "component_run_tasks": attr.label_list(
            doc = "The component run tasks.",
            providers = [FuchsiaWorkflowInfo],
            mandatory = True,
        ),
        "package": attr.label(
            doc = "The package.",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
    },
)

# buildifier: disable=function-docstring
def fuchsia_package_tasks(
        *,
        name,
        package,
        component_run_tags,
        tools = {},
        is_test = False,
        package_repository_name = None,
        disable_repository_name = None,
        test_realm = None,
        enumerate_test_components = False,
        enumerated_component_filter = None,
        retries = None,
        disable_retries_on_failure = None,
        tags = [],
        **kwargs):
    # Disable Bazel sandboxing when running tests to allow ffx run without
    # isolation.
    # This is preferred over using ffx isolation as isolation would cause
    # unfavorable interaction with existing daemons.
    # Examples:
    #  - Port :8083 collisions when a package server is already running under a
    #    different deamon.
    #  - Emulators running under --net user are not visible to ffx daemons
    #    running within an isolate dir.
    if is_test:
        tags = tags + ["no-sandbox", "no-cache"]
    elif enumerate_test_components:
        fail("enumerate_test_components is only allowed when is_test = True.")
    elif test_realm:
        fail("test_realm is only allowed when is_test = True.")

    # Every target beside the default target should be marked as manual,
    # as they are irrelevant to `bazel build //...` and incompatible with
    # `bazel test //...`.
    intermediate_target_tags = tags + ["manual"]

    # Override testonly since it's used to determine test vs non-test rule
    # variant selection for workflows.
    kwargs["testonly"] = is_test

    # For `bazel run :pkg.debug_symbols`.
    debug_symbols_task = verbs.debug_symbols(name)
    fuchsia_task_register_debug_symbols(
        name = debug_symbols_task,
        deps = [package],
        tags = intermediate_target_tags,
        **kwargs
    )

    # For `bazel run :pkg.publish`.
    publish_task = verbs.publish(name)
    anonymous_publish_task = "%s_anonymous" % publish_task
    anonymous_repo_name = "bazel.%s" % normalized_target_name(anonymous_publish_task)
    fuchsia_task_publish(
        name = anonymous_publish_task,
        packages = [package],
        package_repository_name = package_repository_name or anonymous_repo_name,
        tags = intermediate_target_tags,
        **kwargs
    )

    fuchsia_task_ffx(
        name = verbs.delete_repo(anonymous_publish_task),
        arguments = [
            "repository",
            "server",
            "stop",
            anonymous_repo_name,
        ],
        default_argument_scope = "explicit",
        tags = intermediate_target_tags,
        **kwargs
    )

    publish_only_task = "%s_only" % publish_task
    fuchsia_task_publish(
        name = publish_only_task,
        packages = [package],
        package_repository_name = package_repository_name,
        tags = intermediate_target_tags,
        **kwargs
    )
    fuchsia_workflow(
        name = publish_task,
        sequence = [
            debug_symbols_task,
            publish_only_task,
        ],
        tags = intermediate_target_tags,
        **kwargs
    )

    # For `bazel run :pkg.component`.
    component_run_tasks = []
    for run_tag in component_run_tags:
        component_run_task = verbs.custom(run_tag)(name)
        component_run_tasks.append("%s.run_only" % component_run_task)
        fuchsia_task_run_component(
            name = component_run_tasks[-1],
            default_argument_scope = "global",
            repository = package_repository_name or anonymous_repo_name,
            package = package,
            run_tag = run_tag,
            tags = intermediate_target_tags,
            disable_repository = disable_repository_name,
            test_realm = test_realm,
            **kwargs
        )

        fuchsia_workflow(
            name = component_run_task,
            sequence = [
                debug_symbols_task,
                anonymous_publish_task,
                component_run_tasks[-1],
            ] + ([] if package_repository_name else [
                verbs.delete_repo(anonymous_publish_task),
            ]),
            tags = intermediate_target_tags,
            **kwargs
        )

    # For `bazel test :pkg.test_enumerated`.
    if enumerate_test_components:
        enumerated_testing_task = verbs.test_enumerated(name)
        enumerated_testing_subtask = "%s.run_only" % enumerated_testing_task
        fuchsia_task_test_enumerated_components(
            name = enumerated_testing_subtask,
            default_argument_scope = "global",
            repository = package_repository_name or anonymous_repo_name,
            package = package,
            component_name_filter = enumerated_component_filter,
            test_realm = test_realm,
            retries = retries,
            disable_retries_on_failure = disable_retries_on_failure,
            tags = intermediate_target_tags,
            **kwargs
        )

        fuchsia_workflow(
            name = enumerated_testing_task,
            sequence = [
                debug_symbols_task,
                anonymous_publish_task,
                enumerated_testing_subtask,
            ] + ([] if package_repository_name else [
                verbs.delete_repo(anonymous_publish_task),
            ]),
            tags = intermediate_target_tags,
            **kwargs
        )

        # Ensure the following for test packages:
        # 1. `fuchsia_[unit]test_package` (non-prebuilt) packages never offer
        #    the "test enumerated components" workflow. This is because we have
        #    perfect knowledge of the test components that exist in the test
        #    package during analysis phase when it is created within the build
        #    system.
        # 2. `fuchsia_prebuilt_test_package` should always offer the "test
        #    enumarted components" workflow. This is because we cannot know all
        #    of the test components during analysis phase in a non error-prone
        #    manner so it's useful to offer this as an option.
        #    The default `bazel test :pkg` workflow should:
        #    a. Operate exactly like `fuchsia_[unit]test_package`, if users
        #       specify `test_components`.
        #    b. Fallback to `bazel test :pkg.test_enumerated` behavior, if
        #       `test_components` is omitted.
        if not component_run_tasks:
            component_run_tasks = [enumerated_testing_subtask]

    # For `bazel run :pkg.tool`.
    for label, tool in tools.items():
        tool_run_task = _to_verb(label)(name)
        fuchsia_task_run_driver_tool(
            name = "%s.run_only" % tool_run_task,
            default_argument_scope = "global",
            repository = package_repository_name or anonymous_repo_name,
            package = package,
            tool = tool,
            tags = intermediate_target_tags,
            **kwargs
        )

        fuchsia_workflow(
            name = tool_run_task,
            sequence = [
                debug_symbols_task,
                anonymous_publish_task,
                "%s.run_only" % tool_run_task,
            ] + ([] if package_repository_name else [
                verbs.delete_repo(anonymous_publish_task),
            ]),
            tags = intermediate_target_tags,
            **kwargs
        )

    # For `bazel run :pkg.help`.
    help_task = verbs.help(name)
    _fuchsia_package_help(
        name = help_task,
        package = package,
        tools = tools,
        debug_symbols_task = debug_symbols_task,
        publish_task = publish_task,
        top_level_name = name,
        is_test = is_test,
        enumerated_testing_workflow = enumerate_test_components,
        tags = intermediate_target_tags,
        **kwargs
    )

    # For `bazel run :pkg`.
    _fuchsia_package_default_task(
        name = name,
        help_task = help_task,
        debug_symbols_task = debug_symbols_task,
        publish_task = anonymous_publish_task,
        publish_cleanup_task = None if (
            package_repository_name
        ) else verbs.delete_repo(anonymous_publish_task),
        component_run_tasks = component_run_tasks,
        is_test = is_test,
        package = package,
        tags = tags,
        **kwargs
    )
