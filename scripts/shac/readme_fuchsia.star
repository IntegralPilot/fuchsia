# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""SHAC check for README.fuchsia validity."""

load("./common.star", "compiled_tool_path", "os_exec")

def _filter_readme_fuchsia_files(ctx, files):
    allowlist_path = "tools/readme_fuchsia/assets/allowlist.json"
    data = json.decode(str(ctx.io.read_file(allowlist_path)))
    ignored_prefixes = data.get("ignored_prefixes", [])
    ignored_files = data.get("ignored_files", [])

    return [
        f
        for f in files
        if not any([f.startswith(p) for p in ignored_prefixes]) and f not in ignored_files
    ]

def _readme_fuchsia_required_fields(ctx):
    """Runs `readme_fuchsia validate <file>`

    Args:
      ctx: A ctx instance.
    """
    exe = compiled_tool_path(ctx, "readme_fuchsia")

    files = _filter_readme_fuchsia_files(ctx, ctx.scm.affected_files(glob = "README.fuchsia"))
    if not files:
        return

    procs = []
    for f in files:
        findings_file = ctx.io.tempfile("")
        args = [exe, "validate", "-findings_file", findings_file, f]
        procs.append((f, findings_file, os_exec(ctx, args, ok_retcodes = (0, 1))))

    for f, findings_file, proc in procs:
        res = proc.wait()
        raw_findings = str(ctx.io.read_file(findings_file)).strip()
        if raw_findings:
            for item in json.decode(raw_findings) or []:
                ctx.emit.finding(
                    level = item.get("level", "error"),
                    message = item.get("message", ""),
                    filepath = item.get("filepath", f),
                    line = item.get("line"),
                    end_line = item.get("end_line"),
                    col = item.get("col"),
                    end_col = item.get("end_col"),
                    replacements = item.get("replacements"),
                )
        elif res.retcode != 0:
            message = res.stderr.strip() or res.stdout.strip()
            if not message:
                message = "readme_fuchsia validate failed"
            ctx.emit.finding(
                level = "error",
                message = message,
                filepath = f,
            )

def register_readme_fuchsia_checks():
    shac.register_check(shac.check(_readme_fuchsia_required_fields, name = "readme_fuchsia_required_fields"))
