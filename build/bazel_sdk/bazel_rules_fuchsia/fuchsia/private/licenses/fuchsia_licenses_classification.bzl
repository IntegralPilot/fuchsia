# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for classifying OSS licenses."""

def _fuchsia_licenses_classification_impl(ctx):
    out_json = ctx.actions.declare_file(ctx.label.name)

    identify_license_output_json = ctx.actions.declare_file(ctx.label.name + ".identify_license_output.json")

    inputs = [ctx.file.spdx_input, ctx.executable.identify_license]
    arguments = [
        "--spdx_input=%s" % ctx.file.spdx_input.path,
        "--identify_license_bin=%s" % ctx.executable.identify_license.path,
        "--identify_license_output=%s" % identify_license_output_json.path,
        "--output_file=%s" % out_json.path,
        "--quiet",
    ]

    if ctx.attr.default_is_project_shipped:
        arguments.append("--default_is_project_shipped=True")
    if ctx.attr.default_is_notice_shipped:
        arguments.append("--default_is_notice_shipped=True")
    if ctx.attr.default_is_source_code_shipped:
        arguments.append("--default_is_source_code_shipped=True")
    if ctx.attr.allowed_conditions:
        arguments.append("--allowed_conditions")
        arguments.extend(ctx.attr.allowed_conditions)
    if ctx.attr.conditions_requiring_shipped_notice:
        arguments.append("--conditions_requiring_shipped_notice")
        arguments.extend(ctx.attr.conditions_requiring_shipped_notice)
    if ctx.attr.fail_on_disallowed_conditions:
        arguments.append("--fail_on_disallowed_conditions=True")
    if ctx.file.failure_message_preamble:
        inputs.append(ctx.file.failure_message_preamble)
        arguments.append("--failure_message_preamble=%s" % ctx.file.failure_message_preamble.path)
    if ctx.files.policy_override_rules:
        inputs.extend(ctx.files.policy_override_rules)
        arguments.append("--policy_override_rules")
        arguments.extend([f.path for f in ctx.files.policy_override_rules])

    ctx.actions.run(
        progress_message = "Generating license classifications into %s" % out_json.path,
        inputs = inputs,
        outputs = [out_json, identify_license_output_json],
        mnemonic = "FuchsiaLicensesClassification",
        executable = ctx.executable._generate_licenses_classification_tool,
        arguments = arguments,
    )

    return [DefaultInfo(files = depset([out_json]))]

fuchsia_licenses_classification = rule(
    doc = """
Produces a json file with license classification output.

The \\[name\\].json has the following schema:

```
{
    // Dictionary of license_ids : lists of classifications
    "[license_id]":
        [
            // list of named classifications
            {
                "name": str,
                "confidence": float,
                "start_line": int,
                "end_line": int,
            },
            ...
        ],
    ,
    ...
}
```
""",
    implementation = _fuchsia_licenses_classification_impl,
    attrs = {
        "spdx_input": attr.label(
            doc = "The target to aggregate the licenses from.",
            allow_single_file = True,
            mandatory = True,
        ),
        "identify_license": attr.label(
            doc = """The location of the 'identify_license' tool from
https://github.com/google/licenseclassifier/tree/main/tools/identify_license
or a program with a similar I/O. Different organizations should configure
and build identify_license to match their organization OSS compliance policies.
""",
            executable = True,
            allow_single_file = True,
            cfg = "exec",
            mandatory = True,
        ),
        "policy_override_rules": attr.label_list(
            doc = """Condition override rule files""",
            allow_files = True,
            mandatory = False,
            default = [],
        ),
        "default_condition": attr.string(
            doc = "Deprecated",
            mandatory = False,
            default = "",
        ),
        "default_is_project_shipped": attr.bool(
            doc = "Whether by default OSS projects are shipped",
            mandatory = False,
            default = True,
        ),
        "default_is_notice_shipped": attr.bool(
            doc = "Whether by default OSS notices are shipped",
            mandatory = False,
            default = True,
        ),
        "default_is_source_code_shipped": attr.bool(
            doc = "Whether by default OSS source code is shipped",
            mandatory = False,
            default = False,
        ),
        "allowed_conditions": attr.string_list(
            doc = """List of allowed conditions.""",
            mandatory = False,
            default = [],
        ),
        "conditions_requiring_shipped_notice": attr.string_list(
            doc = """Only licenses with the any of the given condition will be shipped as notices.""",
            mandatory = False,
            default = [],
        ),
        "fail_on_disallowed_conditions": attr.bool(
            doc = """The rule will fail if identified licenses map to disallowed conditions.""",
            mandatory = False,
            default = True,
        ),
        "failure_message_preamble": attr.label(
            doc = """A text file that contains a failure message preamble.
The message will be pre-pended to the standard generated failure message,
allowing downstream customers to provide project specific instructions, such
as documentation or persons of contact.""",
            mandatory = False,
            allow_single_file = True,
        ),
        "_generate_licenses_classification_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//fuchsia/tools/licenses:generate_licenses_classification",
        ),
    },
)
