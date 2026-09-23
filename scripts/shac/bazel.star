# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines SHAC checks for Bazel files."""

def _bazel_default_applicable_licenses(ctx):
    """Checks that non-third-party BUILD.bazel files have default_applicable_licenses set."""
    allowlist_path = "build/bazel/shac/allowlists.json"
    data = json.decode(str(ctx.io.read_file(allowlist_path)))
    ignored_prefixes = data.get("ignored_prefixes", [])
    ignored_files = data.get("ignored_files", [])

    for f in ctx.scm.affected_files(glob = ["BUILD.bazel"]):
        should_ignore = False
        if f in ignored_files:
            should_ignore = True
        else:
            for p in ignored_prefixes:
                prefix = p if p.endswith("/") else p + "/"
                if f.startswith(prefix):
                    should_ignore = True
                    break

        if should_ignore:
            continue

        contents = str(ctx.io.read_file(f))

        # Match package( ... default_applicable_licenses = ["//:license"] ... )
        # Using [^)]* to match anything except closing parenthesis to stay within the package call.
        if not ctx.re.allmatches(r"package\([^)#]*default_applicable_licenses\s*=\s*\[\"//:license\"\][^)]*\)", contents):
            ctx.emit.finding(
                level = "error",
                message = "BUILD.bazel files must include `default_applicable_licenses = [\"//:license\"]` within a `package()` call to set the default file-level license.",
                filepath = f,
            )

def register_bazel_checks():
    shac.register_check(shac.check(_bazel_default_applicable_licenses, formatter = False))
