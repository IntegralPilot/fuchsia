# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for defining post processing script."""

load(
    ":providers.bzl",
    "FuchsiaPostProcessingScriptInfo",
)

def _fuchsia_post_processing_script_impl(ctx):
    args = []
    inputs = {}

    if ctx.attr.use_python:
        py_toolchain = ctx.attr._py_toolchain[platform_common.ToolchainInfo]
        if not py_toolchain.py3_runtime:
            fail("A Bazel python3 runtime is required, and none was configured!")

        python3_executable = py_toolchain.py3_runtime.interpreter
        args += [
            "-p",
            python3_executable.path,
        ]
        inputs.update({
            file: file.path
            for file in py_toolchain.py3_runtime.files.to_list() + [python3_executable]
        })

    args.extend(ctx.attr.post_processing_script_args)
    inputs.update(ctx.attr.post_processing_script_inputs)

    return [
        FuchsiaPostProcessingScriptInfo(
            post_processing_script_path = ctx.attr.post_processing_script_path,
            post_processing_script_args = args,
            post_processing_script_inputs = inputs,
        ),
    ]

fuchsia_post_processing_script = rule(
    doc = """Generates post processing script target.""",
    implementation = _fuchsia_post_processing_script_impl,
    provides = [FuchsiaPostProcessingScriptInfo],
    attrs = {
        "post_processing_script_path": attr.string(
            doc = """Path to post processing script. Note: resultant zbi probably can't
            be used with tools like scrutiny, because we won't know how to unpack the zbi
            out of whatever the post-processing script has done to it.""",
        ),
        "post_processing_script_args": attr.string_list(
            doc = "Post processing script arguments",
        ),
        "post_processing_script_inputs": attr.label_keyed_string_dict(
            doc = """Dictionary for artifacts used by post processing script.
            It will be a source -> destination map""",
            allow_files = True,
        ),
        "use_python": attr.bool(
            doc = """Some post processing script requires python binary to execute the python script. Instead of making customer pass in their own python binary and runfiles, python can be provided through python toolchain.""",
        ),
        "_py_toolchain": attr.label(
            default = "@rules_python//python:current_py_toolchain",
            cfg = "exec",
            providers = [DefaultInfo, platform_common.ToolchainInfo],
        ),
    },
)
