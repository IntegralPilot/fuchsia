# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros for defining a Fuchsia component and component manifest."""

load(
    "@fuchsia_rules_common//components:component_manifest.bzl",
    "compile_component_manifest",
)
load(
    "@fuchsia_rules_common//components:fuchsia_component_common.bzl",
    "fuchsia_component_common",
)

def _fx_component_manifest_impl(ctx):
    manifest_in = ctx.file.manifest
    component_name = ctx.attr.component_name or ctx.label.name

    cmc = ctx.executable._cmc_tool

    return compile_component_manifest(
        ctx = ctx,
        cmc_tool = cmc,
        manifest_in = manifest_in,
        component_name = component_name,
        includes = ctx.files.includes,
        include_paths = [".", "sdk/lib"],
    )

fx_component_manifest = rule(
    doc = """Compiles a Fuchsia component manifest (.cml) into a binary component manifest (.cm).

This rule executes the component manifest compiler (cmc) tool to validate
and compile the input manifest file, including any specified dependency shards.
""",
    implementation = _fx_component_manifest_impl,
    attrs = {
        "manifest": attr.label(
            doc = "The component manifest file (.cml) to compile.",
            allow_single_file = [".cml"],
            mandatory = True,
        ),
        "component_name": attr.string(
            doc = "The name of the component. Defaults to the label name of the target.",
        ),
        "includes": attr.label_list(
            doc = """Other manifest shard files (.shard.cml) to include during compilation.
Explicitly setting this is necessary for sandboxed build action execution.""",
            allow_files = [".shard.cml"],
        ),
        "_cmc_tool": attr.label(
            doc = "The path to the component manifest compiler (cmc) tool.",
            default = "@gn_targets//toolchain_host_x64/tools/cmc:cmc",
            executable = True,
            cfg = "exec",
        ),
    },
)

def _fx_component_impl(
        name,
        component_name,
        compiled_manifest,
        deps,
        testonly,
        visibility,
        **kwargs):
    fuchsia_component_common(
        name = name,
        compiled_manifest = compiled_manifest,
        component_name = component_name or name,
        deps = deps,
        testonly = testonly,
        visibility = visibility,

        # Attributes not supported by the in-tree macro.
        moniker = None,
        is_driver = False,
        is_test = False,

        # Forward extra attributes.
        **kwargs
    )

fx_component = macro(
    doc = """Creates a Fuchsia component which can be added to a package.

This macro will take a component manifest and compile it into a form that
is suitable to be included in a package. The component can include any
number of dependencies which will be included in the final package.
""",
    implementation = _fx_component_impl,
    inherit_attrs = fuchsia_component_common,
    attrs = {
        # The behavior is different from that documented for the inherited attribute.
        "component_name": attr.string(
            doc = """The name of the component.

            This value will override the component name value in the `compiled_manifest`.
            Defaults to the `name` of this target.
            """,
            mandatory = False,
        ),

        # TODO(https://fxbug.dev/520207779): Determine whether we need these attributes for platform
        # packages.
        "moniker": None,
        "is_driver": None,
        "is_test": None,
    },
)
