# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""SHAC check for repository license compliance."""

load("./common.star", "compiled_tool_path", "get_fuchsia_dir", "os_exec")

def _should_run_full_validate(affected_files):
    """Returns True if changes require a full repository license audit."""
    if affected_files == None or len(affected_files) > 100:
        return True

    for path, meta in affected_files.items():
        if (
            path.startswith("manifests/") or
            "jiri.lock" in path or
            "tools/check-licenses/assets/" in path
        ):
            return True
        if meta.action == "D" and (
            path.startswith("third_party/") or
            path.startswith("vendor/") or
            path.endswith("README.fuchsia") or
            "LICENSE" in path
        ):
            return True

    return False

def _check_licenses(ctx):
    """Runs `check-licenses` to verify repository license compliance.

    Args:
      ctx: A ctx instance.
    """
    exe = compiled_tool_path(ctx, "check-licenses")
    fuchsia_dir = get_fuchsia_dir(ctx)
    findings_file = ctx.io.tempfile("")

    affected_files = ctx.scm.affected_files()
    if _should_run_full_validate(affected_files):
        cmd = [exe, "validate", "--fuchsia_dir", fuchsia_dir, "-findings_file", findings_file]
    else:
        files = [f for f, meta in affected_files.items() if meta.action != "D"]
        if not files:
            return
        files_file = ctx.io.tempfile("\n".join(files) + "\n")
        cmd = [
            exe,
            "project",
            "check",
            "--fuchsia_dir",
            fuchsia_dir,
            "--fast",
            "--format=json",
            "-findings_file",
            findings_file,
            "-file-list",
            files_file,
        ]

    res = os_exec(
        ctx,
        cmd,
        raise_on_failure = False,
    ).wait()

    raw_findings = str(ctx.io.read_file(findings_file)).strip()
    findings = json.decode(raw_findings) if raw_findings else []
    if findings:
        for f in findings:
            filepath = f.get("filepath")
            if filepath and filepath not in ctx.scm.all_files(glob = filepath):
                filepath = None
            ctx.emit.finding(
                level = f.get("level", "error"),
                message = f.get("message", ""),
                filepath = filepath,
                line = f.get("line") if filepath else None,
                end_line = f.get("end_line") if filepath else None,
                replacements = f.get("replacements") if filepath else None,
            )
    elif res.retcode != 0:
        message = res.stderr.strip() or res.stdout.strip() or ("Execution failed with exit code %d" % res.retcode)
        ctx.emit.finding(
            level = "error",
            message = "License compliance check failed:\n{}".format(message),
        )

def register_check_licenses_checks():
    shac.register_check(shac.check(_check_licenses))
