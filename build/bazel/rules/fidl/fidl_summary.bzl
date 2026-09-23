# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for generating a FIDL API summary."""

visibility("private")

# LINT.IfChange

def _fidl_summary_impl(ctx):
    """Implementation of the fidl_summary rule."""
    summary_file_json = ctx.outputs.output
    json_representation = ctx.file.input

    args = [
        "--fidl-ir-file",
        json_representation.path,
        "--output-file",
        summary_file_json.path,
        "--suppress-empty-library",
    ]

    ctx.actions.run(
        outputs = [summary_file_json],
        inputs = [json_representation],
        executable = ctx.executable._tool,
        arguments = args,
        mnemonic = "FidlApiSummarize",
    )

fidl_summary = rule(
    doc = """Generates a machine-readable, JSON-formatted, FIDL API summary.

For details on the FIDL API summary format, see RFC-0076 at:
https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0076_fidl_api_summaries""",
    implementation = _fidl_summary_impl,
    attrs = {
        "input": attr.label(
            doc = "The FIDL IR file to read.",
            mandatory = True,
            allow_single_file = True,
        ),
        "output": attr.output(
            doc = "The output API summary file to generate.",
            mandatory = True,
        ),
        "_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//tools/fidl/fidl_api_summarize:fidl_api_summarize",
        ),
    },
)

# LINT.ThenChange(//build/fidl/fidl_summary.gni)
