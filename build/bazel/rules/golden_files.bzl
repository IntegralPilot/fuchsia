# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""verify_golden_files() custom rule definition."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@fuchsia_build_info//:args.bzl", "update_goldens")

def _verify_golden_files_impl(ctx):
    if int(bool(ctx.attr.golden_file)) + int(bool(ctx.attr.golden_files)) + int(bool(ctx.attr.comparison_manifest)) != 1:
        fail("Exactly one of `golden_file`, `golden_files`, or `comparison_manifest` must be specified.")
    if ctx.attr.golden_file and len(ctx.attr.candidate_files) != 1:
        fail("`candidate_files` must have one entry when `golden_file` is specified.")
    if ctx.attr.golden_files and len(ctx.attr.candidate_files) != len(ctx.attr.golden_files):
        fail("`candidate_files` and `golden_files` must be the same length.")
    if bool(ctx.attr.candidate_files) == bool(ctx.attr.comparison_manifest):
        fail("Exactly one of `candidate_files` or `comparison_manifest` must be specified.")
    if ctx.attr.comparison_manifest and ctx.attr.formatter_executable:
        fail("Formatter scripts are not supported with `comparison_manifest`.")
    if not ctx.attr.formatter_executable and (ctx.attr.formatter_args or ctx.attr.formatter_extensions or ctx.attr.formatter_inputs):
        fail("Formatter arguments may only be specified with `formatter_executable`.")

    verify_args = ctx.actions.args()
    verify_inputs = []
    verify_tools = [ctx.executable._verify_script]

    output_stamp = ctx.actions.declare_file(ctx.label.name + ".verified")

    verify_args.add("--stamp-file", output_stamp.path)
    verify_args.add("--source-root", ".")  # Paths are relative to execroot.

    if ctx.attr.message:
        verify_args.add("--err-msg=" + ctx.attr.message)
    if ctx.attr.binary:
        verify_args.add("--binary")
    if ctx.attr.only_warn_on_changes:
        verify_args.add("--warn")
    if ctx.attr.golden_dir:
        verify_args.add("--golden-dir=" + ctx.attr.golden_dir)

    execution_requirements = {}
    if update_goldens:
        verify_args.add("--bless")

        # The Bazel sandbox must be disabled to update source files.
        execution_requirements["no-sandbox"] = "1"

        # Disable remoting and caching too since we are modifying local files.
        execution_requirements["no-remote"] = "1"
        execution_requirements["no-cache"] = "1"

    verify_args.add("--label", ctx.label)

    comparison_manifest_file = None

    if ctx.file.comparison_manifest:
        comparison_manifest_file = ctx.file.comparison_manifest

        # TODO(https://fxbug.dev/427998443): When adding support for
        # `zither_golden_files()`, ensure the candidate and golden files listed
        # in the manifest are added to `verify_inputs` or remove support for
        # this mechanism and use the other one.
        fail("`comparison_manifest` is not fully supported yet.")

    else:
        # Generate the comparison manifest from `candidate_files` and `goldens`.
        comparison_manifest_file = ctx.actions.declare_file(ctx.label.name + ".comparisons.json")

        processed_comparisons_for_json = []
        for i, candidate_file in enumerate(ctx.files.candidate_files):
            if ctx.attr.golden_file:
                golden_file = ctx.file.golden_file
            else:
                golden_file = ctx.files.golden_files[i]
            verify_inputs.append(candidate_file)
            verify_inputs.append(golden_file)

            # The base entry for this pair of files. A formatted golden may be added.
            comparison_entry = {
                "candidate": candidate_file.path,
                "golden": golden_file.path,
            }

            # Format the golden file if required.
            if ctx.file.formatter_executable:
                # TODO(https://fxbug.dev/427998443): Validate formatter support.
                fail("Formatter support has not been validated. Test before using.")

                # buildifier: disable=unreachable
                apply_formatter = True
                if ctx.attr.formatter_extensions:
                    golden_ext_with_dot = paths.split_extension(golden_file.path)
                    golden_ext = golden_ext_with_dot[1:] if golden_ext_with_dot and golden_ext_with_dot.startswith(".") else golden_ext_with_dot
                    if golden_ext not in ctx.attr.formatter_extensions:
                        apply_formatter = False

                if apply_formatter:
                    formatted_golden_rel_path = paths.join(ctx.label.name, "formatted_goldens", golden_file.path)
                    formatted_golden_output = ctx.actions.declare_file(formatted_golden_rel_path)

                    # This is unused but required by the script.
                    # TODO(https://fxbug.dev/425931839): Remove this argument
                    # once the script no longer needs to support GN.
                    format_script_depfile = ctx.actions.declare_file(formatted_golden_rel_path + ".d")

                    # format_golden.sh args: depfile, original_golden_path, formatted_golden_output_path, formatter_executable_path, [formatter_args...]
                    format_args = ctx.actions.args()
                    format_args.add(format_script_depfile.path)
                    format_args.add(golden_file.path)
                    format_args.add(formatted_golden_output.path)
                    format_args.add(ctx.executable.formatter_executable.path)
                    if ctx.attr.formatter_args:
                        format_args.add_all(ctx.attr.formatter_args)

                    format_action_inputs = [
                        golden_file,
                        ctx.files.formatter_executable,
                    ]
                    if ctx.files.formatter_inputs:
                        format_action_inputs += ctx.files.formatter_inputs

                    ctx.actions.run(
                        outputs = [formatted_golden_output, format_script_depfile],
                        inputs = format_action_inputs,
                        tools = [ctx.executable.formatter_executable],
                        executable = ctx.executable._format_script,
                        arguments = [format_args],
                        mnemonic = "FormatGoldenFile",
                        progress_message = "Formatting golden: %s" % golden_file.path,
                    )

                    # We don't want to supply the formatted golden under the "golden" key
                    # as the script needs to know the original location in order to be able to
                    # auto-update it when a diff is detected.
                    verify_inputs.append(formatted_golden_output)
                    comparison_entry["formatted_golden"] = formatted_golden_output.path

            processed_comparisons_for_json.append(comparison_entry)

        if not processed_comparisons_for_json:
            fail("No comparisons specified.")

        ctx.actions.write(
            output = comparison_manifest_file,
            content = json.encode_indent(processed_comparisons_for_json, indent = "  "),
        )

    verify_inputs.append(comparison_manifest_file)
    verify_args.add("--comparisons", comparison_manifest_file.path)
    verify_args.add("--paths-relative-to-source-root")

    # Finally, verify all comparisons.
    ctx.actions.run(
        outputs = [output_stamp],
        inputs = verify_inputs,
        tools = verify_tools,
        executable = ctx.executable._verify_script,
        arguments = [verify_args],
        mnemonic = "VerifyGoldenFiles",
        progress_message = "Verifying golden files for %s" % ctx.label.name,
        execution_requirements = execution_requirements,
    )

    # TODO(https://fxbug.dev/427998443): Provide metadata for //:golden_files.

    return [DefaultInfo(files = depset([output_stamp]))]

verify_golden_files = rule(
    implementation = _verify_golden_files_impl,
    attrs = {
        "candidate_files": attr.label_list(
            doc = "List of candidate file labels to be checked against `golden_files`. " +
                  " Must be the same length as `golden_files`. " +
                  "Each file in the list corresponds to the golden file at the same index in `golden_files`." +
                  "Mutually exclusive with `comparison_manifest`.",
            allow_files = True,
            mandatory = False,
        ),
        # Currently, the build fails with a "missing input file" error if the
        # golden file does not exist.
        # TODO(https://fxbug.dev/427998443): Implement developer-friendly
        # behavior similar to that in GN when the golden file does not exist.
        # This may require wrapping this rule in a macro and detecting this
        # before invoking the rule.
        "golden_file": attr.label(
            doc = "Golden file label against which to check `candidate_files`. " +
                  "`candidate_files` must contain exactly one entry. " +
                  "Mutually exclusive with `golden_files` and `comparison_manifest`.",
            allow_single_file = True,
            mandatory = False,
        ),
        "golden_files": attr.label_list(
            doc = "List of golden file labels against which to check `candidate_files`. " +
                  "Must be the same length as `candidate_files`. " +
                  "Each file in the list corresponds to the golden file at the same index in `candidate_files`." +
                  "Mutually exclusive with `golden_file` and `comparison_manifest`.",
            allow_files = True,
            mandatory = False,
        ),
        "comparison_manifest": attr.label(
            doc = "Label pointing to a JSON file describing the comparisons. " +
                  "Mutually exclusive with `candidate_files`, `golden_file`, and `golden_files`.",
            allow_single_file = True,
            mandatory = False,
        ),
        "golden_dir": attr.string(
            doc = "If set, then all golden files must be within this directory. " +
                  "If any other files are present in this directory, the check will fail " +
                  "with instructions to remove obsolete files from this directory.",
            mandatory = False,
        ),
        "binary": attr.bool(
            doc = "If true, files are compared as binary and no diff is shown if there is a mismatch.",
            default = False,
            mandatory = False,
        ),
        "only_warn_on_changes": attr.bool(
            doc = "If true, mismatches are treated as warnings rather than errors.",
            default = False,
            mandatory = False,
        ),
        "message": attr.string(
            doc = "Additional error message to print if files don't match.",
            mandatory = False,
        ),
        "formatter_executable": attr.label(
            doc = "Path to the formatting executable. " +
                  "The formatter takes a file via stdin and outputs its contents to stdout.",
            allow_single_file = True,
            cfg = "exec",
            executable = True,
            mandatory = False,
        ),
        "formatter_args": attr.string_list(
            doc = "List of arguments to pass to the formatter executable." +
                  "Paths must be relative to the workspace",
            mandatory = False,
        ),
        "formatter_extensions": attr.string_list(
            doc = "List of file extensions to which application of the formatter should be limited. " +
                  "An empty list is taken to mean that the formatter should be applied to every golden.",
            mandatory = False,
        ),
        "formatter_inputs": attr.label_list(
            doc = "Additional files that are inputs to the formatter execution.",
            allow_files = True,
            cfg = "exec",
            mandatory = False,
        ),
        "_verify_script": attr.label(
            default = Label("//build/testing:verify_golden_files"),
            executable = True,
            # allow_single_file = True,
            cfg = "exec",
        ),
        "_format_script": attr.label(
            default = Label("//build/testing:format_golden.sh"),
            executable = True,
            allow_single_file = True,
            cfg = "exec",
        ),
    },
)
