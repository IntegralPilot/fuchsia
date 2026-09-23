# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

FORMATTER_MSG = "File not formatted. Run `fx format-code` to fix."

def os_exec(ctx, cmd, *args, **kwargs):
    """Runs `ctx.os.exec()`, validating that the executable path is absolute.
    """
    if not cmd[0].startswith("/"):
        fail("%s is not absolute" % cmd[0])
    return ctx.os.exec(cmd, *args, **kwargs)

def get_fuchsia_dir(ctx):
    rel_fuchsia_dir = ctx.vars.get("fuchsia_dir")
    if rel_fuchsia_dir == ".":
        return ctx.scm.root
    return ctx.scm.root + "/" + rel_fuchsia_dir

def cipd_platform_name(ctx):
    """Returns CIPD's name for the current host platform.

    This is the platform name that appears in most prebuilt paths.
    """
    os = {
        "darwin": "mac",
    }.get(ctx.platform.os, ctx.platform.os)
    arch = {
        "amd64": "x64",
    }.get(ctx.platform.arch, ctx.platform.arch)
    return "%s-%s" % (os, arch)

def get_build_dir(ctx):
    """Returns the path to the build output directory."""
    return get_fuchsia_dir(ctx) + "/" + ctx.vars.get("fuchsia_build_dir")

def compiled_tool_path(ctx, tool_name):
    """Returns the path to a compiled tool in the build directory."""
    build_dir = get_build_dir(ctx)
    tools = json.decode(str(ctx.io.read_file(build_dir + "/tool_paths.json")))
    for tool in tools:
        if (
            tool["name"] == tool_name and
            "%s-%s" % (tool["os"], tool["cpu"]) == cipd_platform_name(ctx)
        ):
            return build_dir + "/" + tool["path"]
    fail("no such tool in tool_paths.json: %s" % tool_name)
