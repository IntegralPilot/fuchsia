# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Build information used in the Bazel product configs."""

load("@fuchsia_build_info//:args.bzl", "build_info_version", "truncate_build_info_commit_date")
load("//build/bazel/rules/python:py_toolchain.bzl", "PY_TOOLCHAIN_ATTRS", "generate_python_build_action")

DEFAULT_PRODUCT_BUILD_INFO = {
    "version": "LABEL(@//build/info:version)",
    "jiri_snapshot": "LABEL(@//build/info:jiri_snapshot)",
    "latest_commit_date": "LABEL(@//build/info:latest_commit_date)",
}

def _gen_latest_date_and_timestamp_impl(ctx):
    # Declare output files.
    stamp_file = ctx.actions.declare_file("minimum_utc_stamp.txt")
    commit_hash = ctx.actions.declare_file("latest_commit_hash.txt")
    commit_date = ctx.actions.declare_file("latest_commit_date.txt")

    # IMPORTANT: Order matters for other rules defined in this file!!
    outputs = [stamp_file, commit_hash, commit_date]

    # Build command line arguments.
    cmd_args = [
        "--input-hash-file",
        ctx.file._commit_hash_file.path,
        "--input-stamp-file",
        ctx.file._commit_stamp_file.path,
        "--timestamp-file",
        stamp_file.path,
        "--date-file",
        commit_date.path,
        "--commit-hash-file",
        commit_hash.path,
    ]

    if truncate_build_info_commit_date:
        cmd_args += ["--truncate"]

    generate_python_build_action(
        ctx = ctx,
        py_script = ctx.file._py_script,
        arguments = cmd_args,
        outputs = outputs,
        inputs = [
            ctx.file._commit_hash_file,
            ctx.file._commit_stamp_file,
        ],
        execution_requirements = {
            # No need to remote this action.
            "local": "1",
        },
    )

    return DefaultInfo(
        files = depset(outputs),
    )

gen_latest_date_and_timestamp = rule(
    implementation = _gen_latest_date_and_timestamp_impl,
    attrs = {
        "_py_script": attr.label(
            allow_single_file = True,
            default = "//build/info:gen_latest_commit_date.py",
        ),
        "_commit_hash_file": attr.label(
            allow_single_file = True,
            default = "//build/info:jiri_generated/integration_commit_hash.txt",
        ),
        "_commit_stamp_file": attr.label(
            allow_single_file = True,
            default = "//build/info:jiri_generated/integration_commit_stamp.txt",
        ),
    } | PY_TOOLCHAIN_ATTRS,
)

def _get_indexed_output_impl(ctx):
    # No need to generate anything, just return a single output file.
    common_outputs = ctx.attr.from_target[DefaultInfo].files.to_list()
    return DefaultInfo(files = depset([common_outputs[ctx.attr._output_index]]))

def _make_get_indexed_output_rule(output_index):
    """Return a rule() value that extracts the n-th output of a different target."""
    return rule(
        implementation = _get_indexed_output_impl,
        attrs = {
            "from_target": attr.label(
                mandatory = True,
                doc = "A gen_latest_date_and_timestamp() target definition.",
            ),
            "_output_index": attr.int(
                default = output_index,
                doc = "Index of file in 'from' target's outputs.",
            ),
        },
    )

get_timestamp_file = _make_get_indexed_output_rule(0)
get_latest_commit_hash = _make_get_indexed_output_rule(1)
get_latest_commit_date = _make_get_indexed_output_rule(2)

# If the `build_info_version` string is empty, define the build_info_version()
# target as an alias to `//build/info:latest_commit_data`, otherwise write
# its content into a file using a custom rule.

def _get_build_info_version_impl(ctx):
    ctx.actions.write(ctx.outputs.output, build_info_version)

_get_build_info_version_rule = rule(
    implementation = _get_build_info_version_impl,
    doc = "A text file describing the current Fuchsia build configuration.",
    attrs = {
        "output": attr.output(
            doc = "Output file",
            mandatory = True,
        ),
    },
)

def get_build_info_version(name):
    """A target that outputs a text file describing the current build configuration."""
    if build_info_version != "":
        _get_build_info_version_rule(name = name, output = "build_info_version.txt")
    else:
        native.alias(
            name = name,
            actual = "//build/info:latest_commit_date",
        )
