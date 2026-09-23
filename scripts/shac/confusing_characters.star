# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

misleading_characters = {
    # smart quotes are misleading
    r"“": '"',
    r"”": '"',
    # en and em dashes are misleading
    r"–": "-",
    r"—": "-",
}

# Lints for confusing characters like smart quotes and en dashes.
# It's better to avoid these so that they don't crop up in code snippets
# and produce confusing failures when developers copy-paste them from
# documentation.
def confusing_characters(ctx):
    for path, meta in ctx.scm.affected_files().items():
        for num, line in meta.new_lines():
            for c, replacement in misleading_characters.items():
                matches = ctx.re.allmatches(c, line)
                if not matches:
                    continue
                for match in matches:
                    ctx.emit.finding(
                        message = "Avoid using confusing characters: {}".format(c),
                        level = "warning",
                        filepath = path,
                        line = num,
                        col = match.offset + 1,
                        end_col = match.offset + 2,
                        replacements = [replacement],
                    )
