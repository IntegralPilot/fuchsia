# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A cc_library generated from a bind library."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load(":fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":providers.bzl", "FuchsiaBindLibraryInfo")

def _codegen_impl(ctx):
    bindc = get_fuchsia_sdk_toolchain(ctx).bindc
    base_path = ctx.attr.name
    name = ctx.attr.library[FuchsiaBindLibraryInfo].name.replace(".", "/").replace("_bindlib", "")
    bind_source = ctx.attr.library[DefaultInfo].files.to_list()[0]

    # The generated header fie.
    header = ctx.actions.declare_file(paths.join(base_path, "bind", name, "cpp", "bind.h"))

    ctx.actions.run(
        executable = bindc,
        arguments = [
            "generate-cpp",
            "--lint",
            "--output",
            header.path,
            bind_source.path,
        ],
        inputs = [bind_source],
        outputs = [header],
        mnemonic = "BindcGenCc",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )

    return [
        DefaultInfo(files = depset([header])),
    ]

# Runs bindc to produce the header file with the constants for the bind_library.
_codegen = rule(
    implementation = _codegen_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "library": attr.label(
            doc = "The bind library to generate code for",
            mandatory = True,
            allow_files = False,
            providers = [FuchsiaBindLibraryInfo],
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS,
)

def fuchsia_bind_cc_library(name, library, deps = [], tags = [], **kwargs):
    """Generates a cc_library() for the given fuchsia_bind_library().

    Args:
      name: Target name. Required.
      library: fuchsia_bind_library() target to generate the code for. Required.
      deps: Additional dependencies.
      tags: Optional tags.
      **kwargs: Remaining args.
    """
    gen_name = "%s_codegen" % name
    _codegen(
        name = gen_name,
        library = library,
    )

    cc_library(
        name = name,
        hdrs = [
            ":%s" % gen_name,
        ],
        # This is necessary in order to locate generated headers.
        strip_include_prefix = gen_name,
        deps = deps,
        tags = tags,
        **kwargs
    )
