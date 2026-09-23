# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for C and C++ files."""

load(
    "./common.star",
    "FORMATTER_MSG",
    "cipd_platform_name",
    "get_fuchsia_dir",
    "os_exec",
)

def _header_guards(ctx):
    """Checks and formats C/C++ header guards."""
    headers = list(ctx.scm.affected_files(glob = [
        "*.h",
        "!/third_party/**",
    ]).keys())
    if not headers:
        return

    fuchsia_dir = get_fuchsia_dir(ctx)
    platform = cipd_platform_name(ctx)
    python_bin = "%s/prebuilt/third_party/python3/%s/bin/python3" % (
        fuchsia_dir,
        platform,
    )
    checker_script = "%s/scripts/shac/check_header_guards.py" % fuchsia_dir

    procs = []
    for h in headers:
        procs.append((
            h,
            os_exec(
                ctx,
                [python_bin, checker_script, "--root", fuchsia_dir, "--emit", h],
                ok_retcodes = (0, 1),
            ),
        ))

    for h, proc in procs:
        res = proc.wait()
        if res.retcode != 0:
            ctx.emit.finding(
                level = "warning",
                message = res.stderr.strip() or ("Header guard issue in %s." % h),
                filepath = h,
            )
        else:
            formatted = res.stdout
            original = str(ctx.io.read_file(h))
            if formatted and formatted != original:
                ctx.emit.finding(
                    level = "warning",
                    message = FORMATTER_MSG,
                    filepath = h,
                    replacements = [formatted],
                )

def register_cpp_checks():
    shac.register_check(shac.check(_header_guards, formatter = True))
