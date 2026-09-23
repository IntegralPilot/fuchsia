# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Main entry point for Fuchsia SHAC checks."""

# keep-sorted start
load("./bazel.star", "register_bazel_checks")
load("./bazel_migration.star", "register_bazel_migration_checks")
load("./bazel_migration_fidl.star", "register_bazel_migration_fidl_checks")
load("./check_licenses.star", "register_check_licenses_checks")
load("./cml.star", "register_cml_checks")
load("./commit_msg.star", "register_commit_msg_checks")
load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "get_fuchsia_dir", "os_exec")
load("./confusing_characters.star", "confusing_characters")
load("./cpp.star", "register_cpp_checks")
load("./dart.star", "register_dart_checks")
load("./dml.star", "register_dml_checks")
load("./docs.star", "register_doc_checks")
load("./fidl.star", "register_fidl_checks")
load("./gn.star", "gn_no_print")
load("./go.star", "register_go_checks")
load("./json.star", "register_json_checks")
load("./keep_sorted.star", "keep_sorted")
load("./mirror_blocklists.star", "register_mirror_blocklists_checks")
load("./owners.star", "register_owners_checks")
load("./python.star", "register_python_checks")
load("./readme_fuchsia.star", "register_readme_fuchsia_checks")
load("./rust.star", "register_rust_checks")
load("./skills.star", "register_skills_checks")
load("./starlark.star", "register_starlark_checks")
load("./underscore_vs_dash.star", "register_underscore_vs_dash_checks")
# keep-sorted end

def bug_urls(ctx):
    """Checks that fuchsia bug URLs are correctly formatted.

    Bug URLs should use the form "https://fxbug.dev/42074375"; the form
    "http://fxb/123456" isn't usable by non-Google employees, and
    "fxbug.dev/42074375" doesn't automatically linkify in most editors.

    Args:
        ctx: A ctx instance.
    """
    correct_format = "https://fxbug.dev/"
    for f, meta in ctx.scm.affected_files().items():
        for num, line in meta.new_lines():
            for match in ctx.re.allmatches(
                r"(https?://)?fxb(ug\.dev)?/(\d+)",
                line,
            ):
                if match.groups[0].startswith(correct_format):
                    continue
                bug_number = match.groups[-1]
                repl = correct_format + bug_number
                end_offset = match.offset + len(match.groups[0])

                # Ignore invalid shortlinks if they're wrapped in square
                # brackets, which likely indicates markdown formatting where the
                # text is a link title rather than the link itself.
                if (match.offset > 0 and line[match.offset - 1] == "[") and (
                    end_offset < len(line) and line[end_offset] == "]"
                ):
                    continue

                ctx.emit.finding(
                    level = "warning",
                    message = "Bug links should use the form %s." % repl,
                    filepath = f,
                    line = num,
                    col = match.offset + 1,
                    end_col = end_offset + 1,
                    replacements = [repl],
                )

def _gn_format(ctx):
    """Runs gn format on .gn and .gni files.

    Args:
        ctx: A ctx instance.
    """
    affected_files = ctx.scm.affected_files(glob = ["*.gn", "*.gni"])
    if not affected_files:
        return

    gn = "%s/prebuilt/third_party/gn/%s/gn" % (get_fuchsia_dir(ctx), cipd_platform_name(ctx))

    result = os_exec(
        ctx,
        [gn, "format", "--dry-run"] + list(affected_files),
        ok_retcodes = [0, 1, 2],
    ).wait()

    lines = result.stdout.splitlines()
    files_to_format = []
    has_errors = False
    for line in lines:
        if not line.strip():
            continue
        if line in affected_files:
            files_to_format.append(line)
        else:
            has_errors = True

    for f in files_to_format:
        formatted_contents = os_exec(
            ctx,
            [gn, "format", "--stdin"],
            stdin = ctx.io.read_file(f),
        ).wait().stdout
        ctx.emit.finding(
            level = "error",
            message = FORMATTER_MSG,
            filepath = f,
            replacements = [formatted_contents],
        )

    # `gn format --dry-run` has three output cases:
    # 1. Prints nothing if the file has the correct formatting.
    # 2. Prints the name of the file if it needs formatting changes.
    # 3. Prints a raw parser error (e.g., 'ERROR at :1:1: ...') to stdout if it fails to parse,
    #    but crucially does NOT print the filename of the broken file.
    #
    # Because of Case 3, we cannot tell from the batch output which file is broken.
    # To identify the broken file(s), we manually track which files successfully parsed but
    # need formatting (Case 2, stored in `files_to_format`).
    #
    # If we detected any parser errors (indicated by `has_errors`), we fall back to running
    # `gn format --stdin` one-by-one on all remaining files (Case 1 and Case 3, excluding
    # `files_to_format`). This allows us to capture the exact parser error and associate it
    # with the correct filename.
    #
    # The optimization here is that if `has_errors` is False, we can completely skip this
    # fallback check for all the Case 1 files that are already correctly formatted.
    if has_errors:
        files_to_check_for_errors = [f for f in affected_files if f not in files_to_format]
        errors = []
        for f in files_to_check_for_errors:
            res = os_exec(
                ctx,
                [gn, "format", "--stdin"],
                stdin = ctx.io.read_file(f),
                ok_retcodes = [0, 1, 2],
            ).wait()
            if res.retcode == 1:
                errors.append("{}:\n  {}".format(f, res.stdout))
        if errors:
            fail("\n" + "\n\n".join(errors))

def register_all_checks():
    """Register all checks that should run.

    Checks must be registered in a callback function because they can only be
    registered by the root shac.star file, not at the top level of any `load`ed
    file.
    """
    shac.register_check(shac.check(_gn_format, formatter = True))
    shac.register_check(keep_sorted)
    shac.register_check(bug_urls)
    shac.register_check(confusing_characters)
    shac.register_check(gn_no_print)

    # keeps-sorted start
    register_bazel_checks()
    register_bazel_migration_checks()
    register_bazel_migration_fidl_checks()
    register_check_licenses_checks()
    register_cml_checks()
    register_commit_msg_checks()
    register_cpp_checks()
    register_dart_checks()
    register_dml_checks()
    register_doc_checks()
    register_fidl_checks()
    register_go_checks()
    register_json_checks()
    register_mirror_blocklists_checks()
    register_owners_checks()
    register_python_checks()
    register_readme_fuchsia_checks()
    register_rust_checks()
    register_skills_checks()
    register_starlark_checks()
    register_underscore_vs_dash_checks()
    # keeps-sorted end
