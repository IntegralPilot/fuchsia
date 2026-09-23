# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "FORMATTER_MSG", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def _gofmt(ctx):
    """Runs gofmt on a Go code base.

    Args:
      ctx: A ctx instance.
    """
    go_files = ctx.scm.affected_files(glob = ["*.go", "!/third_party/**"])
    if not go_files:
        return

    base_cmd = [
        "%s/prebuilt/third_party/go/%s/bin/gofmt" % (
            get_fuchsia_dir(ctx),
            cipd_platform_name(ctx),
        ),
        "-s",  # simplify
    ]

    unformatted = os_exec(ctx, base_cmd + ["-l"] + list(go_files)).wait().stdout.splitlines()
    for f in unformatted:
        new_contents = os_exec(ctx, base_cmd + [f]).wait().stdout
        ctx.emit.finding(
            level = "error",
            message = FORMATTER_MSG,
            filepath = f,
            replacements = [new_contents],
        )

def register_go_checks():
    shac.register_check(shac.check(_gofmt, formatter = True))
