# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("./common.star", "cipd_platform_name", "get_fuchsia_dir", "os_exec")

def _keep_sorted(ctx):
    """Enforces keep-sorted directives in source files.

    Docs: https://github.com/google/keep-sorted#options

    Args:
      ctx: A ctx instance.
    """
    exe = "%s/prebuilt/third_party/keep-sorted/%s/keep-sorted" % (
        get_fuchsia_dir(ctx),
        cipd_platform_name(ctx),
    )

    files = ctx.scm.affected_files().keys()
    findings = []

    # Split up files into batches to avoid exceeding command-line argument
    # length limit.
    batch_size = 20000
    for i in range(0, len(files), batch_size):
        res = os_exec(
            ctx,
            [exe, "--mode=lint"] + files[i:i + batch_size],
            ok_retcodes = (0, 1),
        ).wait()
        if res.retcode == 1:
            if not res.stdout:
                fail(res.stderr)
            findings.extend(json.decode(res.stdout))

    for finding in findings:
        replacements = [
            repl["new_content"]
            for fix in finding["fixes"]
            for repl in fix["replacements"]
            # Only include replacements that apply to the same span as the
            # finding since shac doesn't support replacements on different
            # spans.
            if repl["lines"] == finding["lines"] and
               # Exclude empty replacements that say to delete `start` directive
               # lines without a matching `end` directive line, this is rarely
               # an appropriate fix.
               repl["new_content"]
        ]
        msg = finding["message"]
        if len(replacements) == 1:
            if not msg.endswith("."):
                msg += "."
            msg += " Run `fx format-code` to fix."
        ctx.emit.finding(
            level = "error",
            message = msg,
            filepath = finding["path"],
            line = finding["lines"]["start"],
            end_line = finding["lines"]["end"],
            replacements = replacements,
        )

keep_sorted = shac.check(_keep_sorted, formatter = True)
