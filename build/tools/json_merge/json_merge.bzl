# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for merging JSON files."""

def _json_merge_impl(ctx):
    out_filename = ctx.attr.out if ctx.attr.out else ctx.label.name
    out_file = ctx.actions.declare_file(out_filename)

    args = ctx.actions.args()
    for src in ctx.files.srcs:
        args.add("--input", src.path)
    args.add("--output", out_file.path)

    if ctx.attr.deep_merge:
        args.add("--deep")
    if ctx.attr.minify:
        args.add("--minify")
    if ctx.attr.relaxed_input:
        args.add("--relaxed")

    ctx.actions.run(
        outputs = [out_file],
        inputs = ctx.files.srcs,
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "JsonMerge",
        progress_message = "Merging JSON into %s" % out_file.short_path,
    )

    return [DefaultInfo(files = depset([out_file]))]

json_merge = rule(
    implementation = _json_merge_impl,
    doc = "Merge one or more JSON files.",
    attrs = {
        "srcs": attr.label_list(
            doc = "One or more JSON files to merge.",
            mandatory = True,
            allow_files = True,
        ),
        "out": attr.string(
            doc = "Output filename. Defaults to target name.",
        ),
        "relaxed_input": attr.bool(
            doc = "Enables relaxed input parsing.",
            default = False,
        ),
        "deep_merge": attr.bool(
            doc = "Whether to merge nested objects.",
            default = False,
        ),
        "minify": attr.bool(
            doc = "Whether to minify the result.",
            default = False,
        ),
        "_tool": attr.label(
            default = "//build/tools/json_merge",
            executable = True,
            cfg = "exec",
        ),
    },
)
