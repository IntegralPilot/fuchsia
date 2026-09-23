# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

BROKEN_INCLUDE_MSG = "File does not exist, can't be included."

def owners(ctx):
    """Validates include statements in owners files.

    Args:
      ctx: A ctx instance.
    """
    owners_files = ctx.scm.affected_files(glob = "OWNERS")

    procs = []
    for f in owners_files:
        procs.append(
            (f, os_exec(ctx, [
                "%s/prebuilt/third_party/python3/%s/bin/python3" % (
                    get_fuchsia_dir(ctx),
                    cipd_platform_name(ctx),
                ),
                "scripts/shac/broken_include_owners.py",
                f,
            ])),
        )

    for f, proc in procs:
        res = proc.wait()
        for finding in json.decode(res.stdout):
            for line in finding["lines"]:
                ctx.emit.finding(
                    level = "error",
                    filepath = f,
                    message = BROKEN_INCLUDE_MSG,
                    line = line,
                )

def register_owners_checks():
    shac.register_check(shac.check(owners, formatter = False))
