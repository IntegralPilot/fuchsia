# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for validating JSON files."""

def _validate_json_action_impl(ctx):
    stamp_file = ctx.actions.declare_file(ctx.label.name + "." + ctx.attr.stamp_extension)

    args = ctx.actions.args()
    if ctx.attr.allow_comments:
        args.add("--json5")

    args.add(ctx.file.schema.path)
    args.add(ctx.file.data.path)
    args.add(stamp_file.path)

    ctx.actions.run(
        outputs = [stamp_file],
        inputs = [ctx.file.data, ctx.file.schema] + ctx.files.sources,
        executable = ctx.executable._valico_tool,
        arguments = [args],
        mnemonic = "ValidateJSON",
        progress_message = "Validating JSON %s" % ctx.file.data.short_path,
    )

    return [DefaultInfo(files = depset([stamp_file]))]

_validate_json_action = rule(
    implementation = _validate_json_action_impl,
    doc = "Validate a JSON file against a JSON schema.",
    attrs = {
        "data": attr.label(
            doc = "JSON file to validate.",
            mandatory = True,
            allow_single_file = True,
        ),
        "schema": attr.label(
            doc = "Schema to use for validation.",
            mandatory = True,
            allow_single_file = True,
        ),
        "sources": attr.label_list(
            doc = "Additional schema files referenced by schema.",
            allow_files = True,
        ),
        "allow_comments": attr.bool(
            doc = "If True, the data file may contain JSON5-style comments.",
            default = False,
        ),
        "stamp_extension": attr.string(
            default = "json_validated",
        ),
        "_valico_tool": attr.label(
            default = "//build/tools/json_validator:json_validator_valico",
            executable = True,
            cfg = "exec",
        ),
    },
)

def _validate_json_impl(
        name,
        data,
        schema,
        sources,
        allow_comments,
        **kwargs):
    _validate_json_action(
        name = name,
        data = data,
        schema = schema,
        sources = sources,
        allow_comments = allow_comments,
        stamp_extension = "json_validated",
        **kwargs
    )

validate_json = macro(
    doc = "Validate a JSON file against a JSON schema.",
    inherit_attrs = _validate_json_action,
    implementation = _validate_json_impl,
    attrs = {
        "stamp_extension": None,
    },
)

def _validate_json5_impl(
        name,
        data,
        schema,
        sources,
        **kwargs):
    _validate_json_action(
        name = name,
        data = data,
        schema = schema,
        sources = sources,
        allow_comments = True,
        stamp_extension = "json5_validated",
        **kwargs
    )

validate_json5 = macro(
    doc = "Validate a JSON5 file against a JSON schema.",
    inherit_attrs = _validate_json_action,
    implementation = _validate_json5_impl,
    attrs = {
        "allow_comments": None,
        "stamp_extension": None,
    },
)
