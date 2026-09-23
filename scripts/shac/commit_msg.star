# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def _check_commit_message(ctx):
    """Lints Git commit messages on the current branch against style guidelines."""
    commits = ctx.scm.commits()
    if not commits:
        return

    fuchsia_dir = get_fuchsia_dir(ctx)
    python_bin = "%s/prebuilt/third_party/python3/%s/bin/python3" % (
        fuchsia_dir,
        cipd_platform_name(ctx),
    )
    checker_path = "%s/scripts/shac/commit_msg_checker.py" % fuchsia_dir

    for commit in commits:
        msg_file = ctx.io.tempfile(commit.message, name = "commit_message.txt")
        res = os_exec(
            ctx,
            [python_bin, checker_path, "--message-file", msg_file, "--json"],
            raise_on_failure = False,
        ).wait()

        if not res.stdout or not res.stdout.strip().startswith("["):
            continue

        findings = json.decode(res.stdout)
        for finding in findings:
            msg = finding.get("message", "")
            kwargs = {
                "level": finding.get("level", "warning"),
                "commit": commit,
            }
            if finding.get("line"):
                kwargs["line"] = finding["line"]
            ctx.emit.commit_message_finding(message = msg, **kwargs)

def register_commit_msg_checks():
    shac.register_check(shac.check(_check_commit_message, name = "commit_msg"))
