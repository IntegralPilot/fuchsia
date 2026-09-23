# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

def _verify_file_path_impl(ctx):
    files_list = ctx.attr.target[DefaultInfo].files.to_list()
    if len(files_list) < 1:
        fail("The target's `DefaultInfo` must have at least one file.")

    actual_file_path = files_list[0].short_path
    if actual_file_path != ctx.attr.expected_file_path:
        fail("The actual file path (`%s`) does not match the expected file path (`%s`)." %
             (actual_file_path, ctx.attr.expected_file_path))
    return []

verify_file_path = rule(
    doc = "Verifies that the actual file path of a target matches the expected short file path." +
          "Compares the short file path of the first file in the `target`'s " +
          "`[DefaultInfo].files` with the `expected_file_path` attribute. " +
          "The `target` must have at least one file in `DefaultInfo.files`.",
    implementation = _verify_file_path_impl,
    attrs = {
        "target": attr.label(mandatory = True),
        "expected_file_path": attr.string(mandatory = True),
    },
)
