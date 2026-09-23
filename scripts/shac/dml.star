# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for DML files."""

load("./common.star", "FORMATTER_MSG", "compiled_tool_path", "os_exec")

def _dml_format(ctx):
    """Runs `cmc format` on .dml files.

    Args:
      ctx: A ctx instance.
    """
    exe = compiled_tool_path(ctx, "cmc")
    procs = []
    for f in ctx.scm.affected_files(glob = "*.dml"):
        procs.append((f, os_exec(ctx, [exe, "format", f])))

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

def register_dml_checks():
    shac.register_check(shac.check(_dml_format, formatter = True))
