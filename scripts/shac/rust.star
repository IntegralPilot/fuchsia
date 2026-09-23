# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "./common.star",
    "FORMATTER_MSG",
    "cipd_platform_name",
    "compiled_tool_path",
    "get_build_dir",
    "get_fuchsia_dir",
    "os_exec",
)

def _clippy(ctx):
    """Parses Clippy linter results produced by the build."""
    files = list(ctx.scm.affected_files(glob = "*.rs"))
    if not files:
        return
    exe = compiled_tool_path(ctx, "clippy-reporter")
    res = os_exec(ctx, [
        exe,
        "-checkout-dir",
        get_fuchsia_dir(ctx),
        "-build-dir",
        get_build_dir(ctx),
        "-files-json",
        ctx.io.tempfile(json.encode(files)),
    ]).wait()

    for finding in json.decode(res.stdout):
        end_col = finding["end_col"]
        if finding["line"] == finding["end_line"] and finding["col"] >= finding["end_col"]:
            # TODO(olivernewman): Remove this debug statement and handling once
            # col >= end_col instances are fixed.
            print(
                "WARNING: Clippy finding has col >= end_col: %s",
                finding,
            )  # allow-print
            end_col = None
        ctx.emit.finding(
            message = finding["message"],
            level = "warning",
            filepath = finding["path"],
            line = finding["line"],
            end_line = finding["end_line"],
            col = finding["col"],
            end_col = end_col,
            replacements = finding["replacements"],
        )

def _rustfmt(ctx):
    """Runs rustfmt on a Rust code base.

    Args:
      ctx: A ctx instance.
    """
    rust_files = ctx.scm.affected_files(glob = [
        "*.rs",
        "!/third_party/**",
        # fidlgen_banjo Rust templates have an ".rs" extension but are not
        # valid Rust.
        "!/src/devices/tools/fidlgen_banjo/src/backends/templates/rust",
    ])
    if not rust_files:
        return

    base_cmd = [
        "%s/prebuilt/third_party/rust/%s/bin/rustfmt" % (
            get_fuchsia_dir(ctx),
            cipd_platform_name(ctx),
        ),
        "--config-path",
        "rustfmt.toml",
        "--unstable-features",
        "--skip-children",
    ]

    res = os_exec(ctx, base_cmd + ["--check", "--files-with-diff"] + list(rust_files), ok_retcodes = [0, 1]).wait()
    unformatted = res.stdout.splitlines()
    if res.retcode and not unformatted:
        fail("rustfmt failed:\n%s" % res.stderr)

    procs = []
    for f in unformatted:
        filepath = f[len(ctx.scm.root) + 1:]
        procs.append((
            filepath,
            os_exec(ctx, base_cmd + ["--emit", "stdout", filepath]),
        ))

    for filepath, proc in procs:
        output = proc.wait().stdout

        # First two lines are file name and a blank line.
        formatted = "\n".join(output.split("\n")[2:])
        ctx.emit.finding(
            # Switch to "error" if it's decided that rustfmt should be enforced
            # in presubmit.
            level = "warning",
            message = FORMATTER_MSG,
            filepath = filepath,
            replacements = [formatted],
        )

def register_rust_checks():
    shac.register_check(shac.check(_clippy))
    shac.register_check(shac.check(_rustfmt, formatter = True))
