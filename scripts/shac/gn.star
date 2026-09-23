# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

def gn_no_print(ctx):
    """Warns if .gn or .gni files contain print() statements."""
    for path, meta in ctx.scm.affected_files(glob = ["*.gn", "*.gni"]).items():
        for num, line in meta.new_lines():
            if line.strip().startswith("#"):
                continue

            # Match print( but ignore commented lines
            matches = ctx.re.allmatches(r"(print\s*\()", line)
            if matches:
                ctx.emit.finding(
                    message = "Avoid print() in GN files. It pollutes stdout and breaks automated tools (like gndoc). Consider using temporary prints and removing them before landing.",
                    level = "warning",
                    filepath = path,
                    line = num,
                    col = matches[0].offset + 1,
                )
                break  # Only one finding per file to reduce noise.
