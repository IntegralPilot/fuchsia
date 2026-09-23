# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("//build/bazel/debug_symbols:aspects.bzl", _generate_manifest_aspect = "generate_manifest")

def _multi_compilation_mode_transition_impl(settings, attr):
    return {
        "Debug build": {"//command_line_option:compilation_mode": "dbg"},
        "Fastbuild build": {"//command_line_option:compilation_mode": "fastbuild"},
        "Optimized build": {"//command_line_option:compilation_mode": "opt"},
    }

multi_compilation_mode_transition = transition(
    implementation = _multi_compilation_mode_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _multi_compilation_mode_filegroup_impl(ctx):
    # Return a DefaultInfo value that points to all files from all dependency variants.
    # There is no point in running such a target, so do not try to compute
    # an all_runfiles value.
    all_files = depset([], transitive = [d[DefaultInfo].files for d in ctx.attr.srcs])
    return [
        DefaultInfo(files = all_files),
    ]

multi_compilation_mode_filegroup = rule(
    doc = """A filegroup-like rule that builds its 'srcs' in multiple compilation modes.

    This rule builds 'srcs' three times, in "dbg", "fastbuild" and "opt" modes,
    then combines all their output files in its DefaultInfo provider.
    """,
    implementation = _multi_compilation_mode_filegroup_impl,
    attrs = {
        "srcs": attr.label_list(
            doc = "A list of labels to targets that will be built in all compilation modes.",
            default = [],
            cfg = multi_compilation_mode_transition,
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
)

def _debug_symbols_manifest_impl(ctx):
    # Just re-export the manifest itself.
    if ctx.attr.output:
        output = ctx.outputs.output
    else:
        output = ctx.actions.declare_file(ctx.target.label.name)

    dep_outputs = ctx.attr.dep[OutputGroupInfo]
    manifest = dep_outputs.debug_symbol_manifest.to_list()[0]
    ctx.actions.symlink(output = output, target_file = manifest)
    return [
        DefaultInfo(files = depset([output])),
        OutputGroupInfo(
            debug_symbol_files = dep_outputs.debug_symbol_files,
        ),
    ]

debug_symbols_manifest = rule(
    doc = """Generate a debug symbols manifest from transitive dependencies of a given target.""",
    implementation = _debug_symbols_manifest_impl,
    attrs = {
        "output": attr.output(
            doc = "Optional output file name.",
            mandatory = False,
        ),
        "dep": attr.label(
            doc = "Top-level dependency to extract debug symbols from.",
            mandatory = True,
            providers = [OutputGroupInfo],
            aspects = [_generate_manifest_aspect],
        ),
    },
)
