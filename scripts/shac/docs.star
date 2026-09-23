# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "compiled_tool_path", "get_fuchsia_dir", "os_exec")

def _doc_checker(ctx):
    """Runs the doc-checker tool."""
    if ctx.scm.root != get_fuchsia_dir(ctx):
        # doc-checker is only relevant for fuchsia.git.
        return

    affected_files = set(ctx.scm.affected_files())

    # If a Markdown change is present (including a deletion of a markdown file),
    # check the entire project.
    if not ctx.scm.affected_files(glob = "*.md"):
        return

    exe = compiled_tool_path(ctx, "doc-checker")
    res = os_exec(ctx, [exe, "--json", "--root", get_fuchsia_dir(ctx)], ok_retcodes = [0, 1]).wait()
    findings = json.decode(res.stdout, default = None)
    if not findings and res.retcode:
        fail("doc-checker failed:\n%s" % res.stderr)
    for finding in findings:
        abspath = finding["doc_line"]["file_name"]
        msg = finding["message"]
        if finding["help_suggestion"]:
            msg += "\n\n" + finding["help_suggestion"]
        msg += "\n\nRun `fx doc-checker --local-links-only` to reproduce."
        filepath = abspath[len(ctx.scm.root) + 1:]
        level = {
            "Info": "notice",
            "Warning": "warning",
            "Error": "error",
        }[finding["level"]]

        # Normally we should only propagate findings if the referenced file is
        # affected to avoid spamming users with non-blocking warnings about
        # files they didn't touch. But errors may occur in unaffected files
        # (e.g. deleting a file may make links in unaffected files invalid) so
        # we should always propagate errors.
        if filepath in affected_files or level == "error":
            ctx.emit.finding(
                filepath = filepath,
                line = finding["doc_line"]["line_num"],
                level = level,
                message = msg,
            )

def _mdlint(ctx):
    """Runs mdlint."""
    rfc_dir = "docs/contribute/governance/rfcs/"
    if not ctx.scm.affected_files(glob = "/docs/contribute/governance/rfcs/**"):
        return
    mdlint = compiled_tool_path(ctx, "mdlint")
    res = os_exec(
        ctx,
        [
            mdlint,
            "--json",
            "--root-dir",
            rfc_dir,
            "--enable",
            "all",
            "--filter-filenames",
            rfc_dir,
        ],
        ok_retcodes = [0, 1],
    ).wait()
    for finding in json.decode(res.stderr):
        if finding["path"] not in ctx.scm.affected_files():
            continue
        ctx.emit.finding(
            level = "warning",
            message = finding["message"],
            filepath = finding["path"],
            line = finding["start_line"],
            end_line = finding["end_line"],
            col = finding["start_char"] + 1,
            end_col = finding["end_char"] + 1,
        )

def _codelinks(ctx):
    """Checks for certain malformatted links in source code."""

    # TODO(olivernewman): Files under //docs should generally reference other
    # documentation files by path (e.g. "//docs/foo/bar.md") rather than URL,
    # with some exceptions for reference docs.
    for f, meta in ctx.scm.affected_files(glob = ["!/docs/**"]).items():
        for num, line in meta.new_lines():
            for match in ctx.re.allmatches(
                r"(https?://)?fuchsia.googlesource.com/fuchsia/\+/(refs/heads/)?\w+/docs/(?P<path>\S+)\.md",
                line,
            ):
                repl = "https://fuchsia.dev/fuchsia-src/" + match.groups[-1]
                ctx.emit.finding(
                    level = "warning",
                    message = (
                        "Documentation links should point to fuchsia.dev rather than " +
                        "fuchsia.googlesource.com. Consider changing this to %s." % repl
                    ),
                    filepath = f,
                    line = num,
                    col = match.offset + 1,
                    end_col = match.offset + 1 + len(match.groups[0]),
                    replacements = [repl],
                )

def _rfcmeta(ctx):
    """Validates RFC metadata."""
    files = ctx.scm.affected_files(glob = "/docs/contribute/governance/rfcs/**")
    if not files:
        return
    exe = compiled_tool_path(ctx, "rfcmeta")
    res = os_exec(ctx, [
        exe,
        "-checkout-dir",
        get_fuchsia_dir(ctx),
    ] + list(files)).wait()

    for finding in json.decode(res.stdout):
        ctx.emit.finding(
            message = finding["message"],
            level = "warning",
            filepath = finding["path"],
            line = finding["line"],
            end_line = finding["end_line"],
            col = finding["col"],
            end_col = finding["end_col"],
            replacements = finding["replacements"],
        )

def register_doc_checks():
    shac.register_check(_codelinks)
    shac.register_check(shac.check(
        _doc_checker,
        # TODO(olivernewman): doc-checker has historically been run from `fx
        # format-code` even though it's not a formatter and doesn't write
        # results back to disk. Determine whether anyone depends on doc-checker
        # running with `fx format-code`, and unset `formatter = True`.
        formatter = True,
    ))
    shac.register_check(_mdlint)
    shac.register_check(_rfcmeta)
