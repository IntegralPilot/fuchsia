# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for Bazel and other Starlark files."""

load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def _get_starlark_files(ctx):
    return list(ctx.scm.affected_files(glob = [
        "BUILD",
        "WORKSPACE",
        "*.BUILD",
        "*.WORKSPACE",
        "*.star",
        "*.bzl",
        "*.bazel",
        "*.bzlmod",
        "*.cquery",
        "!/third_party/**",
    ]))

def _buildifier(ctx):
    """Checks Starlark/Bazel file formatting using buildifier."""
    starlark_files = _get_starlark_files(ctx)
    if not starlark_files:
        return

    base_cmd = [
        "%s/prebuilt/third_party/buildifier/%s/buildifier" % (
            get_fuchsia_dir(ctx),
            cipd_platform_name(ctx),
        ),
        "-lint=off",
    ]

    res = os_exec(
        ctx,
        base_cmd + ["-mode=check"] + starlark_files,
        ok_retcodes = (0, 4),
    ).wait()
    if res.retcode == 0:
        return

    lines = res.stderr.splitlines()
    suffix = " # reformat"

    tempfiles = {}
    for line in lines:
        if not line.endswith(suffix):
            continue
        filepath = line[:-len(suffix)]

        # Buildifier doesn't have a dry-run mode that prints the formatted file
        # to stdout, so copy each file to a temporary file and format the
        # temporary file in-place to obtain the formatted result.
        tempfiles[filepath] = ctx.io.tempfile(
            ctx.io.read_file(filepath),
            name = filepath,
        )

    os_exec(ctx, base_cmd + tempfiles.values()).wait()

    for filepath, temp in tempfiles.items():
        formatted = ctx.io.read_file(temp)
        ctx.emit.finding(
            level = "error",
            filepath = filepath,
            message = FORMATTER_MSG,
            replacements = [str(formatted)],
        )

def _buildifier_lint(ctx):
    """Lints Starlark/Bazel files using buildifier."""
    starlark_files = _get_starlark_files(ctx)
    if not starlark_files:
        return

    res = os_exec(
        ctx,
        [
            "%s/prebuilt/third_party/buildifier/%s/buildifier" % (
                get_fuchsia_dir(ctx),
                cipd_platform_name(ctx),
            ),
            "-lint=warn",
            "-format=json",
            "-mode=check",
        ] + starlark_files,
        ok_retcodes = (0, 4),
    ).wait()

    if not res.stdout:
        return

    parsed = json.decode(res.stdout)

    for file_result in parsed.get("files", []):
        for warning in file_result.get("warnings", []):
            level = "warning"

            # docstring-related warnings are not that important, so treat them as notices.
            if "docstring" in warning["category"]:
                level = "notice"

            if warning["category"] == "load":
                # Unused imports are completely unnecessary, so treat them as
                # errors.
                level = "error"

            ctx.emit.finding(
                level = level,
                filepath = file_result["filename"],
                message = warning["message"],
                line = warning.get("start", {}).get("line"),
                col = warning.get("start", {}).get("column"),
                end_line = warning.get("end", {}).get("line"),
                end_col = warning.get("end", {}).get("column"),
            )

def _fuchsia_shac_style_guide(ctx):
    """Enforces shac conventions that are specific to the fuchsia project."""
    procs = []
    for f in ctx.scm.affected_files(glob = "*.star"):
        procs.append(
            (f, os_exec(ctx, [
                "%s/prebuilt/third_party/python3/%s/bin/python3" % (
                    get_fuchsia_dir(ctx),
                    cipd_platform_name(ctx),
                ),
                "scripts/shac/fuchsia_shac_style_guide.py",
                f,
            ])),
        )

    for f, proc in procs:
        res = proc.wait()
        for finding in json.decode(res.stdout):
            ctx.emit.finding(
                level = "error",
                filepath = f,
                message = finding["message"],
                line = finding["line"],
                col = finding["col"],
                end_line = finding["end_line"],
                end_col = finding["end_col"],
            )

def register_starlark_checks():
    shac.register_check(shac.check(_buildifier, formatter = True))
    shac.register_check(shac.check(_buildifier_lint))
    shac.register_check(shac.check(_fuchsia_shac_style_guide))
