# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for JSON and JSON5 files."""

load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "compiled_tool_path", "get_fuchsia_dir", "os_exec")

def _json5_format(ctx):
    """Runs `formatjson5` on .json5 files.

    Args:
      ctx: A ctx instance.
    """
    exe = compiled_tool_path(ctx, "formatjson5")
    json5_files = ctx.scm.affected_files(glob = [
        "*.json5",
        "*.persist",
        "*.triage",
    ])

    procs = []
    for f in json5_files:
        procs.append((f, os_exec(ctx, [exe, f])))

    for f, proc in procs:
        formatted = proc.wait().stdout
        original = str(ctx.io.read_file(f))
        if formatted != original:
            ctx.emit.finding(
                level = "warning",
                message = FORMATTER_MSG,
                filepath = f,
                replacements = [formatted],
            )

def _json_format(ctx):
    """Runs `json_format.py` on .json and .config files.

    Args:
      ctx: A ctx instance.
    """
    json_files = ctx.scm.affected_files(glob = [
        "*.json",
        "*.config",
        "!/third_party/**",
        "!*.golden.json",
        "!*_pb2.json",
    ])
    if not json_files:
        return

    fuchsia_dir = get_fuchsia_dir(ctx)
    platform = cipd_platform_name(ctx)
    python_bin = "%s/prebuilt/third_party/python3/%s/bin/python3" % (
        fuchsia_dir,
        platform,
    )

    procs = []
    for f in json_files:
        temp = ctx.io.tempfile(ctx.io.read_file(f))
        procs.append((
            f,
            temp,
            os_exec(
                ctx,
                [python_bin, "scripts/shac/json_format.py", "--no-sort-keys", "--quiet", temp],
                ok_retcodes = (0, 1),
            ),
        ))

    for f, temp, proc in procs:
        res = proc.wait()
        if res.retcode == 0:
            formatted = str(ctx.io.read_file(temp))
            original = str(ctx.io.read_file(f))
            if formatted != original:
                ctx.emit.finding(
                    level = "warning",
                    message = FORMATTER_MSG,
                    filepath = f,
                    replacements = [formatted],
                )
        else:
            ctx.emit.finding(
                level = "warning",
                message = "Could not format %s: invalid JSON syntax" % f,
                filepath = f,
            )

def register_json_checks():
    shac.register_check(shac.check(_json5_format, formatter = True))
    shac.register_check(shac.check(_json_format, formatter = True))
