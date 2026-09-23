# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule and provider for host tests."""

load(
    "@rules_cc//cc/common:debug_package_info.bzl",
    "DebugPackageInfo",
)
load("@rules_rust//rust:rust_common.bzl", "CrateInfo")
load("//build/bazel/rules:current_platform_info.bzl", "CurrentPlatformInfo")
load(
    "//build/bazel/rules/host_tests:host_test_data.bzl",
    "CollectedFuchsiaHostTestDataInfo",
    "collect_fuchsia_host_test_data_aspect",
)
load(
    "//build/bazel/rules/python:py_toolchain.bzl",
    "PY_TOOLCHAIN_ATTRS",
    "generate_python_build_action",
)

# Set this to True to enable debug print() statements during analysis.
_DEBUG = False

FuchsiaHostTestInfo = provider(
    doc = "Provider for Bazel host tests visible to Fuchsia test runners (`fx test` and `botanist`).",
    fields = {
        "test_label": "The canonical Bazel label of the host_test() target. Used by `fx test` " +
                      "to rebuild the test on demand.",
        "test_launcher": "A File value for the test launcher script.",
        "test_runtime_dir": "A string path the test runtime directory.",
        "test_runtime_deps_json": "A File value for the runtime_deps.json file for the test.",
        "unstripped_binary": "A File value for the unstripped binary (or None).",
        "os": "The OS of the test, using Fuchsia conventions",
        "cpu": "The CPU of the test, using Fuchsia conventions.",
        "list_cases_argument": "Optional command line argument to list test cases.",
    },
)

def _host_test_impl(ctx):
    # This generates three outputs:
    #
    #  - a `foo` launcher shell script, which changes the current directory to `foo.runtime_dir`
    #    then invokes the entry point (foo.runtime_dir/<binary>) with the test arguments.
    #
    #  - a `foo.runtime_dir` directory that contains symlinks to the binary entry point and
    #    its runfiles. However, the runfiles manifest in it has been adjusted to contain
    #    target paths that are relative to `foo.runtime_dir`.
    #
    #  - a `foo.runtime_deps.json` file that contains the runtime dependencies of the test,
    #    where all paths are relative to the Ninja build directory. This will be used by the
    #    tests.json entry for this test.

    launcher = ctx.actions.declare_file(ctx.attr.name)
    runtime_dir = ctx.actions.declare_directory(ctx.attr.name + ".runtime_dir")
    test_runtime_deps_json = ctx.actions.declare_file(ctx.attr.name + ".runtime_deps.json")

    outputs = [launcher, runtime_dir, test_runtime_deps_json]

    binary_info = ctx.attr.binary[DefaultInfo]
    binary_runfiles_manifest = binary_info.files_to_run.runfiles_manifest
    binary_repo_mapping_manifest = binary_info.files_to_run.repo_mapping_manifest

    # Get the FuchsiaHostTestDataInfo from the aspect on the binary and the data dependencies,
    # and write them to a single file. This will be processed to add new runtime dependencies
    # that do not appear in the test's runfiles manifest, as they should be accessed directly
    # by the test binaries, with paths relative to the test runtime directory.
    #
    # Collect the source files to add them to this target's runfiles though, to allow
    # running it with `bazel run` and `bazel test` properly.
    host_test_data_manifest = ctx.actions.declare_file(ctx.attr.name + ".host_test_data_manifest.json")

    # host_test_data_info is a list of FuchsiaHostTestDataInfo providers from the binary
    # and the test's data dependencies.
    host_test_data_infos = depset(transitive = [
        target[CollectedFuchsiaHostTestDataInfo].infos
        for target in [ctx.attr.binary] + ctx.attr.data
        if CollectedFuchsiaHostTestDataInfo in target
    ]).to_list()

    host_test_data_sources = set()
    for info in host_test_data_infos:
        host_test_data_sources.update(info.files.values())
    host_test_data_runtime_files = sorted(host_test_data_sources)

    ctx.actions.write(
        output = host_test_data_manifest,
        content = json.encode([
            {
                "label": str(info.label),
                "files": {
                    dest_path: source.path
                    for dest_path, source in info.files.items()
                },
            }
            for info in host_test_data_infos
        ]),
        #mnemonic = "FuchsiaHostTestDataManifest",
    )

    host_test_wrapper_template = ctx.file.host_test_wrapper_template

    inputs = (
        binary_info.files.to_list() +
        binary_info.default_runfiles.files.to_list() +
        ctx.files.data +
        ctx.files._python_modules +
        host_test_data_runtime_files
    ) + [
        binary_runfiles_manifest,
        binary_repo_mapping_manifest,
        host_test_data_manifest,
        host_test_wrapper_template,
    ] + ctx.files.host_test_wrapper_generator_deps

    entry_point = binary_info.files_to_run.executable

    # `args` is always added implicitly by Bazel for any rule whose name ends with `_test`.
    # However, because these values are treated specially (i.e. they are not stored in providers)
    # do not use them here, in favor of `test_args`.
    if ctx.attr.args:
        fail("Do not use `args` for this rule. Use `test_args` instead.")

    test_args = []
    for arg in ctx.attr.test_args:
        loc_expanded = ctx.expand_location(arg, ctx.attr.data)
        test_args.append(ctx.expand_make_variables(
            "args",
            loc_expanded,
            {},
        ))

    runfiles = ctx.runfiles(
        files = ctx.files.data,
    ).merge_all(
        [binary_info.default_runfiles] +
        [target[DefaultInfo].default_runfiles for target in ctx.attr.data],
    )

    if _DEBUG:
        def files_list_dump(name, files):
            content = "{} files count={}:\n".format(name, len(files))
            for f in files:
                content += "  {}\n".format(f.path)
            content += "\n"
            return content

        print("launcher path: {}".format(launcher.path))
        print("runtime_dir path: {}".format(runtime_dir.path))
        print("test_args: {}".format(test_args))
        print(files_list_dump("runfiles", runfiles.files.to_list()))
        print(files_list_dump("host_test_data_runtime_files", host_test_data_runtime_files))

    # Ensure $(location <label>) expressions are expanded for the generator arguments.
    generator_args = [
        ctx.expand_location(arg, ctx.attr.host_test_wrapper_generator_deps)
        for arg in ctx.attr.host_test_wrapper_generator_args
    ]

    test_label = Label(ctx.attr.test_label) if ctx.attr.test_label else ctx.label

    generate_python_build_action(
        ctx = ctx,
        py_script = ctx.file._generate_host_test_wrapper,
        arguments = [
            "--entry-point={}".format(entry_point.path),
            "--entry-runfiles-manifest={}".format(binary_runfiles_manifest.path),
            "--test-label={}".format(test_label),
            "--output-launcher={}".format(launcher.path),
            "--output-runtime-dir={}".format(runtime_dir.path),
            "--output-test-runtime-deps-json={}".format(test_runtime_deps_json.path),
            "--bazel-execroot={}".format(ctx.label.workspace_root),
            "--host-test-data-manifest={}".format(host_test_data_manifest.path),
            "--host-test-wrapper-template={}".format(host_test_wrapper_template.path),
        ] + [
            "--data-runfile={}".format(f.path)
            for f in runfiles.files.to_list()
        ] + [
            "--test-arg={}".format(arg)
            for arg in test_args
        ] + generator_args,
        outputs = outputs,
        inputs = inputs,
        # Don't run in a sandbox to ensure the current path is the execroot. This is required
        # to compute the path to the Ninja build directory inside the script.
        execution_requirements = {
            "local": "1",
            "no-sandbox": "1",
        },
    )

    runfiles = ctx.runfiles(
        files = [launcher, runtime_dir] + host_test_data_runtime_files,
    ).merge(runfiles)

    current_platform = ctx.attr._current_platform[CurrentPlatformInfo]

    unstripped_file = ctx.file.unstripped_binary
    if not unstripped_file:
        if DebugPackageInfo in ctx.attr.binary:
            unstripped_file = ctx.attr.binary[DebugPackageInfo].unstripped_file
        elif CrateInfo in ctx.attr.binary:
            unstripped_file = ctx.attr.binary[CrateInfo].output
        else:
            unstripped_file = entry_point

    return [
        DefaultInfo(
            files = depset(outputs),
            runfiles = runfiles,
            executable = launcher,
        ),
        FuchsiaHostTestInfo(
            test_label = test_label,
            test_launcher = launcher,
            test_runtime_dir = runtime_dir.path,
            test_runtime_deps_json = test_runtime_deps_json,
            unstripped_binary = unstripped_file,
            os = current_platform.os,
            cpu = current_platform.cpu,
            list_cases_argument = ctx.attr.list_cases_argument,
        ),
    ]

host_test = rule(
    implementation = _host_test_impl,
    doc = """Create a Bazel host test for Fuchsia.

    Use this rule to create a host test that can be run with `fx test` or `botanist` on infra.

    Regular Bazel test targets are designed to be run from the Bazel execroot only, while the
    Fuchsia test runners invoke the tests from the Ninja build directory, or its equivalent on
    infra test bots. Another issue is that Bazel test attributes (passed through the `args`
    attribute of test rules like `cc_test()` or `py_test()`) are not stored in the generated
    test artifacts. Instead `bazel run` or `bazel test` extract them from the build definition
    before invoking the corresponding test entry point. And regretably there is no way to
    properly query for these arguments (these are never stored in providers).

    This rule solves both problems by ensuring that it produces a test artifact that can be
    invoked from any location, and which hard-codes the test arguments passed in the build
    definition. It also generates an auxiliary file required by the Fuchsia test runners to
    locate the test's runtime dependencies.

    The resulting target *is* also a Bazel test that can be invoked locally using
    `fx bazel test --config=host <label>`.

    Runtime dependencies are specified with the 'data' attribute. As a convenience, any
    data dependencies declared through `host_test_data_xxx()` targets will appear at a fixed
    path in the test's runtime directory. This lets the test binary to access them easily.

    Otherwise, test binaries can still rely on runfiles to access their runtime dependencies,
    but this requires linking a third-party runfiles library, and using hard-coded rlocation
    paths.

    Both ways to declare and access runtime dependencies are supported both with
    `bazel test` and with Fuchsia test runners.
    """,
    test = True,
    executable = True,
    attrs = {
        "binary": attr.label(
            doc = "The test binary to wrap. This can be any executable target, including a Bazel " +
                  "test target, as long as it doesn't have its own `args` attribute values.",
            mandatory = True,
            aspects = [collect_fuchsia_host_test_data_aspect],
        ),
        "unstripped_binary": attr.label(
            doc = "Optional label to the unstripped test binary (if different from binary).",
            allow_single_file = True,
        ),
        "test_label": attr.string(
            doc = "The test label as it will appear in tests.json. This defaults to the current " +
                  "target's label, and should only be overriden for specific edge cases, for " +
                  "example if foo_test is an alias() target that uses select() to choose a " +
                  "different test target based on the current build configuration. The aliased " +
                  "targets should set test_label to 'foo_test' for readability and to avoid " +
                  "breaking existing workflows. " +
                  "IMPORTANT: This attribute must be a string to avoid dependency cycles. It " +
                  "will always be expanded as a full label by the rule though.",
        ),
        "test_args": attr.string_list(
            doc = "Arguments to pass to the test binary. Do *not* use `args` for this purpose.",
            default = [],
        ),
        "data": attr.label_list(
            doc = "Data files to include at runtime. Collected from runfiles and " +
                  "host_test_data_xxx() target definitions reachable from this label list.",
            default = [],
            allow_files = True,
            aspects = [collect_fuchsia_host_test_data_aspect],
        ),
        "list_cases_argument": attr.string(
            doc = "Optional command argument to list test cases. Only used by host_py_test()",
            default = "",
        ),
        "_generate_host_test_wrapper": attr.label(
            doc = "Internal script used to generate the final test wrapper script.",
            default = "//build/bazel/scripts:generate_host_test_wrapper.py",
            allow_single_file = True,
        ),
        "host_test_wrapper_template": attr.label(
            doc = "Template file for the generated test wrapper script.",
            default = "//build/bazel:templates/template.host_test_wrapper.sh",
            allow_single_file = True,
        ),
        "host_test_wrapper_generator_args": attr.string_list(
            doc = "Extra arguments to pass to the test wrapper generator script. " +
                  "This can include $(location <label>) expressions, if these are labels " +
                  "listed in host_test_wrapper_generator_deps.",
            default = [],
        ),
        "host_test_wrapper_generator_deps": attr.label_list(
            doc = "Dependencies of the test wrapper generator invocation.",
            default = [],
        ),
        "_python_modules": attr.label_list(
            doc = "Python modules imported by _host_test_wrapper.",
            allow_files = True,
            default = [
                "//build/bazel/scripts:build_utils.py",
                "//build/bazel/scripts:runfiles_utils.py",
            ],
        ),
        "_current_platform": attr.label(
            providers = [CurrentPlatformInfo],
            default = "@//build/bazel:current_platform",
        ),
    } | PY_TOOLCHAIN_ATTRS,
)
