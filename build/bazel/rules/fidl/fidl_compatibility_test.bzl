# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules and macros for FIDL compatibility tests."""

load("@fuchsia_build_info//:args.bzl", "update_goldens")
load(":fidl_ir.bzl", "fidl_ir")
load(":fidl_summary.bzl", "fidl_summary")

visibility("private")

# LINT.IfChange(run_compatibility_test)
def _fidl_api_compatibility_check_impl(ctx):
    inputs = [ctx.file.current_file]

    if bool(ctx.attr.golden_file) == bool(ctx.attr.golden_file_source_path):
        fail("Exactly one of `golden_file` and `golden_file_source_path` must be set.")

    if ctx.attr.golden_file:
        inputs.append(ctx.file.golden_file)
        golden_file_path = ctx.file.golden_file.path
    else:
        if ctx.attr.policy != "update_golden":
            fail("`golden_file_source_path` can only be set when `policy` is 'update_golden'.")

        # `golden_file_source_path` is a string rather than a Bazel Target, so
        # it cannot be an input. As a result, this target will not be rebuilt if
        # the golden file changes.

        # `golden_file_source_path` must be an absolute path so there is no ambiguity.
        if not ctx.attr.golden_file_source_path.startswith("//"):
            fail("`golden_file_source_path` must start with '//'")

        # The path must be relative to the source directory.
        golden_file_path = ctx.attr.golden_file_source_path.removeprefix("//")

    stamp_file = ctx.actions.declare_file(ctx.label.name + ".verified")

    args = ctx.actions.args()
    args.add("--api-level", ctx.attr.target_api_level)
    args.add("--golden", golden_file_path)
    args.add("--current", ctx.file.current_file.path)
    args.add("--stamp", stamp_file.path)
    args.add("--fidl_api_diff_path", ctx.executable._fidl_api_diff.path)
    args.add("--policy", ctx.attr.policy)

    execution_requirements = {}
    if ctx.attr.policy == "update_golden":
        # The Bazel sandbox must be disabled to update source files.
        execution_requirements["no-sandbox"] = "1"
        execution_requirements["no-remote"] = "1"
        execution_requirements["no-cache"] = "1"

    ctx.actions.run(
        outputs = [stamp_file],
        inputs = inputs,
        executable = ctx.executable._test_script,
        arguments = [args],
        mnemonic = "FidlApiCompatibilityTest" + ctx.attr.target_api_level,
        progress_message = "Verifying FIDL API compatibility for %s" % ctx.label.name,
        execution_requirements = execution_requirements,
    )

    return [DefaultInfo(files = depset([stamp_file]))]

# The name of non-test rules cannot end in `_test`.
_fidl_api_compatibility_check = rule(
    doc = """Compares the `current_file` and `golden_file` API summary JSON files using `fidl_api_diff`.

    When using the "update_golden" `policy` and potentially generating new
    golden files, use 'golden_file_source_path' instead of 'golden_file'. For
    all other use cases, use 'golden_files' exclusively.
    This is a work-around for the fact that Bazel does not support labels
    pointing to nonexistent files.
    """,
    implementation = _fidl_api_compatibility_check_impl,
    attrs = {
        "target_api_level": attr.string(
            doc = "The API level for which the files were generated.",
            mandatory = True,
        ),
        "current_file": attr.label(
            doc = "The current API summary JSON file.",
            mandatory = True,
            allow_single_file = True,
        ),
        "golden_file": attr.label(
            doc = "The expected API summary JSON file." +
                  "Exactly one of this or `golden_file_source_path` must be set.",
            mandatory = False,
            allow_single_file = True,
        ),
        "golden_file_source_path": attr.string(
            doc = """The absolute source path of the expected API summary JSON file.

            Use this to allow a new golden file to be written if it does not exist.
            Since the string is not converted to a Target, it cannot be an
            input, and changes to it will not cause a rebuild.

            May only be set when `policy == 'update_golden'`. Exactly one of
            this or `golden_file` must be set.
            """,
            mandatory = False,
        ),
        "policy": attr.string(
            doc = "The policy to apply.",
            mandatory = True,
        ),
        "_test_script": attr.label(
            default = "//sdk/ctf/build/scripts:fidl_api_compatibility_test",
            executable = True,
            cfg = "exec",
        ),
        "_fidl_api_diff": attr.label(
            default = "//tools/fidl/fidl_api_diff:fidl_api_diff",
            executable = True,
            cfg = "exec",
        ),
    },
)
# LINT.ThenChange(//build/testing/fidl_api_compatibility_test.gni:run_compatibility_test)

# LINT.IfChange(compatibility_test)
def fidl_compatibility_test(
        *,
        name,
        library_name,
        fidl_library_target_name,
        api_level,
        srcs,
        deps,
        goldens_dir,
        fidlc_versioned_arg,
        experimental_flags,
        experimental_checks,
        excluded_checks,
        testonly,
        visibility):
    """A FIDL compatibility test for a single API level.

    Args:
        name: Name of the compatibility test target.
        library_name: Name of the FIDL library.
        api_level: The API level string (e.g., "28", "NEXT", "HEAD").
        fidl_library_target_name: Name of the `fidl_library()` target.
        srcs: List of `.fidl` source files.
        deps: List of labels of other FIDL libraries on which this library depends.
        goldens_dir: Directory containing goldens.
        fidlc_versioned_arg: The value of the `versioned` argument to pass to fidlc.
        experimental_flags: List of experimental `fidlc` features to enable.
        experimental_checks: List of `fidl-lint` check IDs to include (by
                passing the command line flag `-x some-check-id` for each value).
        excluded_checks: List of `fidl-lint` check IDs to ignore (by passing the
                command line flag `-e some-check-id` for each value).
        testonly: Standard meaning.
        visibility: Standard meaning.
    """
    if not ((fidlc_versioned_arg == "fuchsia") or
            (fidlc_versioned_arg == "fuchsia:HEAD")):
        fail("Library '%s' has an unexpected `versioned` arg value ('%s')." % (
            library_name,
            fidlc_versioned_arg,
        ))

    fidl_ir_target_name = name + "_fidl_ir"
    fidl_ir(
        name = fidl_ir_target_name,
        library_name = library_name,
        fidl_library_target_name = fidl_library_target_name,
        srcs = srcs,
        deps = deps,
        available = ["fuchsia:%s" % api_level],
        versioned = fidlc_versioned_arg,
        experimental_flags = experimental_flags,
        experimental_checks = experimental_checks,
        excluded_checks = excluded_checks,
        testonly = testonly,
        visibility = ["//visibility:private"],
        subdirectory = api_level,
        # We do not need to lint the sources again or validate the IR JSON that
        # is not used by any other target. The per-API-level builds will
        # validate the JSON.
        skip_linting_and_validation = True,
    )

    if api_level == "HEAD":
        # For "HEAD", we don't run the compatibility test.
        # Declare a target named `name` that just wraps the `fidl_ir` target,
        # ensuring that the sources can be built at .
        native.filegroup(
            name = name,
            srcs = [fidl_ir_target_name],
            testonly = testonly,
            visibility = visibility,
        )
    else:
        summary_target_name = name + "_summary_json"
        fidl_summary(
            name = summary_target_name,
            input = fidl_ir_target_name,
            output = "%s/%s.api_summary.json" % (api_level, library_name),
            testonly = testonly,
            visibility = ["//visibility:private"],
        )

        # Determine the policy.
        if update_goldens:
            policy = "update_golden"
        elif api_level == "NEXT":
            policy = "ack_changes"
        else:
            policy = "no_changes"

        # The golden path is passed differently depending on the value of
        # `update_goldens`. See `_fidl_api_compatibility_check()` for details.
        # Build a file path or a Label as appropriate then pass it as the
        # appropriate attribute.
        # Note: The `golden_path` file must be visible to the FIDL rules package
        # because this runs inside the `_fidl_library()` symbolic macro. This is
        # different from the `api_file_path` file because its Label is passed to
        # the symbolic macro.
        file_separator = "/" if update_goldens else ":"
        golden_path = "%s/%s%s%s.api_summary.json" % (
            goldens_dir,
            api_level,
            file_separator,
            library_name,
        )

        _fidl_api_compatibility_check(
            name = name,
            target_api_level = api_level,
            current_file = summary_target_name,
            golden_file = None if update_goldens else golden_path,
            golden_file_source_path = golden_path if update_goldens else None,
            policy = policy,
            testonly = testonly,
            visibility = visibility,
        )

# LINT.ThenChange(//build/fidl/fidl_library.gni:compatibility_tests)
