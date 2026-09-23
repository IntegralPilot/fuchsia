# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for generating and validating FIDL IR."""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@fuchsia_build_info//:args.bzl", "runtime_supported_api_levels")
load("//build/json:validate_json.bzl", "validate_json")
load(":providers.bzl", "FidlLibraryInfo")

visibility("private")

# LINT.IfChange(available_default)
def _get_available(ctx):
    if ctx.attr.available:
        return ctx.attr.available

    api_level = ctx.attr._current_api_level[BuildSettingInfo].value

    if api_level == "PLATFORM":
        # FIDL directly supports targeting multiple API levels. "PLATFORM" is a
        # meta-level that refers to the set of all supported API levels.
        return ["fuchsia:" + ",".join(runtime_supported_api_levels)]
    else:
        return ["fuchsia:" + api_level]

# LINT.ThenChange(//build/fidl/fidl_library.gni:available_default)

def _fidlc_impl(ctx):
    library_name = ctx.attr.library_name

    file_prefix = "%s/" % (ctx.attr.subdirectory) if ctx.attr.subdirectory else ""
    file_basename = file_prefix + ctx.attr.fidl_library_target_name
    response_file = ctx.actions.declare_file("%s.args" % file_basename)
    libraries_file = ctx.actions.declare_file("%s.libraries" % file_basename)
    json_representation = ctx.actions.declare_file("%s.fidl.json" % file_basename)

    dep_libraries = [dep[FidlLibraryInfo].libraries_file for dep in ctx.attr.deps]
    srcs_depset = depset(
        direct = ctx.files.srcs,
        transitive = [dep[FidlLibraryInfo].srcs_depset for dep in ctx.attr.deps],
    )

    response_file_args = ctx.actions.args()
    response_file_args.add_all([
        "--out-response-file",
        response_file.path,
        "--out-libraries",
        libraries_file.path,
        "--json",
        json_representation.path,
        "--name",
        library_name,
    ])
    response_file_args.add_all("--sources", ctx.files.srcs)

    if dep_libraries:
        response_file_args.add_all("--dep-libraries", dep_libraries)

    if ctx.attr.versioned:
        response_file_args.add("--versioned", ctx.attr.versioned)

    for available_value in _get_available(ctx):
        response_file_args.add("--available", available_value)

    for flag in ctx.attr.experimental_flags:
        response_file_args.add("--experimental", flag)

    ctx.actions.run(
        executable = ctx.executable._gen_response_file_script,
        arguments = [response_file_args],
        inputs = ctx.files.srcs + dep_libraries,
        outputs = [response_file, libraries_file],
        mnemonic = "GenFidlResponseFile",
    )

    ctx.actions.run(
        executable = ctx.executable._fidlc,
        arguments = ["@" + response_file.path],
        inputs = [response_file] + srcs_depset.to_list(),
        outputs = [json_representation],
        mnemonic = "Fidlc",
    )

    return [
        DefaultInfo(files = depset([json_representation])),
        FidlLibraryInfo(
            name = library_name,
            srcs_depset = srcs_depset,
            libraries_file = libraries_file,
        ),
    ]

_fidlc = rule(
    doc = "Runs the FIDL compiler to generate the FIDL IR.",
    implementation = _fidlc_impl,
    attrs = {
        "library_name": attr.string(
            doc = "Name of the FIDL library.",
            mandatory = True,
        ),
        "fidl_library_target_name": attr.string(
            doc = "Name of the `fidl_library()` target. Used in the name of some generated files.",
            mandatory = True,
        ),
        "srcs": attr.label_list(
            doc = "List of `.fidl` source files.",
            mandatory = True,
            allow_files = True,
            allow_empty = False,
        ),
        "deps": attr.label_list(
            doc = "List of labels of other fidlc targets on which this library depends.",
            mandatory = False,
            providers = [FidlLibraryInfo],
        ),
        "available": attr.string_list(
            doc = "See `fidl_library()`.",
            mandatory = True,
        ),
        "versioned": attr.string(
            doc = "See `fidl_library()`.",
        ),
        "experimental_flags": attr.string_list(
            doc = "A list of experimental fidlc features to enable.",
        ),
        "subdirectory": attr.string(
            doc = "Optional subdirectory for the output files.",
        ),
        "_fidlc": attr.label(
            doc = "The FIDL compiler.",
            default = "@//tools/fidl/fidlc:fidlc",
            executable = True,
            cfg = "exec",
        ),
        "_gen_response_file_script": attr.label(
            default = "//build/fidl:gen_response_file",
            executable = True,
            cfg = "exec",
        ),
        "_current_api_level": attr.label(
            default = "@//build/bazel/versioning:api_level",
        ),
    },
)

# LINT.IfChange(lint)

def _is_exempt_from_linting(package, excluded_checks):
    """
    Returns True if the given FIDL library is exempt from linting.

    Args:
        package: Package path of the FIDL library.
        excluded_checks: List of check IDs to exclude from linting.
    """

    # Don't lint FIDL libraries used to test FIDL itself.
    #
    # Unlike GN, where `/*` is used to include all subdirectories, this
    # implementation uses an exact match of the target's package path.
    # It's possible subdirectories of the packages below will need to be added.
    _fidl_test_packages = [
        "//sdk/lib/fidl/cpp/tests",
        "//sdk/testing/fidl",
        "//sdk/testing/fidl/protocols_tests",
        "//sdk/testing/fidl/types_tests",
        "//src/devices/tools/fidlgen_banjo/tests/fidl",
        "//src/lib/fidl/c/coding_tables_tests",
        "//src/lib/fidl/c/walker_tests",
        "//src/lib/fidl/llcpp/tests",
        "//src/lib/fidl/rust/external_tests",
        "//src/tests/benchmarks/fidl/benchmark_suite",
        "//src/tests/fidl",
        "//tools/fidl/fidlc/testdata",
    ]

    package_path = "//" + package

    if not excluded_checks and package_path in _fidl_test_packages:
        return True

    # TODO(https://fxbug.dev/381163466): Fix lint warnings in vendor repos.
    if not excluded_checks and package_path.startswith("//vendor/"):
        return True

    return False

def _fidl_lint_impl(ctx):
    stamp_file = ctx.actions.declare_file(ctx.attr.fidl_library_target_name + ".linted")

    executable = ctx.executable._fidl_lint

    args = ctx.actions.args()

    # By default, run fidl-lint. The stamp part of the command is added below.
    command = "{fidl_lint} $@ && ".format(
        fidl_lint = executable.path,
    )

    # TODO(https://fxbug.dev/381096879): Implement NOOP logic based on the package name.
    # In GN, some directories skip linting by passing ":" as the tool which is a NOOP.
    # We should implement a similar check here using ctx.label.package.
    if (_is_exempt_from_linting(ctx.label.package, ctx.attr.excluded_checks)):
        # NOOP - Nothing to lint. Skip running fidl-lint but touch the stamp
        # file, which will be added to `command` below.
        command = ""
    else:
        if ctx.attr.excluded_checks:
            # Cause `fidl-lint` to return an error if any excluded check is no
            # longer required. Excluded checks are only allowed if the target
            # files still violate those checks.
            # After updating the FIDL files to resolve a lint error, remove the
            # check ID from the `excluded_checks` list in the `fidl_library()`
            # target to prevent the same lint errors from creeping back in.
            args.add("--must-find-excluded-checks")

            for excluded_check in ctx.attr.excluded_checks:
                args.add("-e", excluded_check)

        for experimental_check in ctx.attr.experimental_checks:
            args.add("-x", experimental_check)

        for flag in ctx.attr.experimental_flags:
            args.add("--experimental", flag)

        for src in ctx.files.srcs:
            args.add(src)

    command += "touch {stamp}".format(
        stamp = stamp_file.path,
    )

    ctx.actions.run_shell(
        inputs = ctx.files.srcs,
        outputs = [stamp_file],
        tools = [executable],
        arguments = [args],
        command = command,
        mnemonic = "FidlLint",
        progress_message = "Linting %{label}",
    )

    return [DefaultInfo(files = depset([stamp_file]))]

_fidl_lint = rule(
    implementation = _fidl_lint_impl,
    attrs = {
        "fidl_library_target_name": attr.string(
            doc = "Name of the `fidl_library()` target. Used in the name of some generated files.",
            mandatory = True,
        ),
        "srcs": attr.label_list(
            allow_files = True,
            mandatory = True,
        ),
        "experimental_checks": attr.string_list(
            doc = "List of `fidl-lint` check IDs to include (by passing the " +
                  "command line flag `-x some-check-id` for each value).",
        ),
        "excluded_checks": attr.string_list(
            doc = "List of `fidl-lint` check IDs to ignore (by passing the " +
                  "command line flag `-e some-check-id` for each value).",
        ),
        "experimental_flags": attr.string_list(
            doc = "A list of experimental fidlc features to enable.",
        ),
        "_fidl_lint": attr.label(
            default = Label("//tools/fidl/fidlc:fidl-lint"),
            executable = True,
            cfg = "exec",
        ),
    },
)
# LINT.ThenChange(//build/fidl/fidl_library.gni:lint)

def _validated_ir_file_impl(ctx):
    return [
        # Allow the target to be used as the IR file.
        ctx.attr.unvalidated_file[DefaultInfo],
        # Pass through the `FidlLibraryInfo` provider so this target can be used
        # as a `deps` by `_fidlc()`.
        ctx.attr.unvalidated_file[FidlLibraryInfo],
        # Ensure the `validation_targets` are built.
        OutputGroupInfo(_validation = depset(ctx.files.validation_targets)),
    ]

_validated_ir_file = rule(
    doc = "Ensures `validation_targets` are built and returns.",
    implementation = _validated_ir_file_impl,
    attrs = {
        "unvalidated_file": attr.label(
            mandatory = True,
            providers = [FidlLibraryInfo],
        ),
        "validation_targets": attr.label_list(
            doc = "The build dependencies",
            mandatory = True,
            allow_files = False,
        ),
    },
)

def fidl_ir(
        *,
        name,
        fidl_library_target_name,
        srcs,
        deps,
        experimental_checks,
        excluded_checks,
        testonly,
        visibility,
        experimental_flags = [],
        subdirectory = None,
        skip_linting_and_validation = False,
        **kwargs):
    """Compiles a FIDL library to IR and returns the validated IR JSON file.

    Args:
        name: Standard meaning.
        fidl_library_target_name: Name of the `fidl_library()` target.
                    Used in the name of some generated files.
        srcs: List of `.fidl` source files.
        deps: List of labels of other FIDL libraries on which this library depends.
        experimental_checks: List of `fidl-lint` check IDs to include (by passing
                    the command line flag `-x some-check-id` for each value)
        excluded_checks: List of `fidl-lint` check IDs to ignore (by passing
                    the command line flag `-e some-check-id` for each value)
        skip_linting_and_validation: Whether to skip linting and JSON validation.
        testonly: Standard meaning.
        visibility: Standard meaning.
        subdirectory: Optional subdirectory for the output files.

        **kwargs: Arguments to pass to the underlying `_fidlc()` rule.
    """
    fidlc_target_name = "%s_fidlc" % name
    _fidlc(
        name = fidlc_target_name,
        fidl_library_target_name = fidl_library_target_name,
        srcs = srcs,
        experimental_flags = experimental_flags,
        # IMPORTANT: The deps must be a label list that was passed to the
        # top-most symbolic macro in order for visibility to be checked
        # correctly. The reason for this is that label strings defined within
        # a symbolic macro will have their visibility checked against the
        # package containing the symbolic macro rather than the package
        # containing the BUILD.bazel file. Since targets defined within a
        # symbolic macro are visible to the package containing that macro, any
        # string labels added here that reference a target defined within the
        # symbolic macro (specifically, `name` below) would be visible to this
        # target regardless of the `visibility` passed to `fidl_library()`.
        # See https://fxbug.dev/446911800.
        deps = deps,
        subdirectory = subdirectory,
        testonly = testonly,
        visibility = ["//visibility:private"],
        **kwargs
    )

    if skip_linting_and_validation:
        # Declare a target named `name` that just wraps the `fidlc` target.
        native.filegroup(
            name = name,
            srcs = [fidlc_target_name],
            testonly = testonly,
            visibility = visibility,
        )
    else:
        lint_target_name = "%s_lint_source_files" % name
        _fidl_lint(
            name = lint_target_name,
            fidl_library_target_name = fidl_library_target_name,
            srcs = srcs,
            experimental_checks = experimental_checks,
            excluded_checks = excluded_checks,
            experimental_flags = experimental_flags,
            testonly = testonly,
            visibility = ["//visibility:private"],
        )

        validate_json_target_name = "%s_validate_ir_json" % name
        validate_json(
            name = validate_json_target_name,
            data = fidlc_target_name,
            schema = "//tools/fidl/fidlc:schema.json",
            testonly = testonly,
            visibility = ["//visibility:private"],
        )

        # IMPORTANT: The name of this target must be the the same as the name that
        # will be used in the `deps` of other FIDL libraries so that `deps` can be
        # used unmodified as explained above.
        _validated_ir_file(
            name = name,
            unvalidated_file = fidlc_target_name,
            validation_targets = [lint_target_name, validate_json_target_name],
            testonly = testonly,
            visibility = visibility,
        )
