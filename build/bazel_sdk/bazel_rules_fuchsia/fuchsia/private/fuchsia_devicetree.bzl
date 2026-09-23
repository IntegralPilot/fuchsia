# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines a devicetree to be built into a devicetree blob."""

load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")
load(":fuchsia_transition.bzl", "fuchsia_transition")
load(":providers.bzl", "FuchsiaDeviceTreeSegmentInfo")
load(":utils.bzl", "PREPROCESS_FILE_ATTRS", "preprocess_file")

FUCHSIA_DEVICETREE_ATTR = {
    "dtcflags": attr.string_list(
        doc = "Flags to be passed to dtc compiler",
    ),
} | PREPROCESS_FILE_ATTRS

def _fuchsia_devicetree_impl(ctx):
    source = ctx.file.source

    order = "postorder"
    includes_sets = []
    headers = []
    file_sets = []

    for dep in ctx.attr.deps:
        if FuchsiaDeviceTreeSegmentInfo in dep:
            includes_sets.append(dep[FuchsiaDeviceTreeSegmentInfo].includes)
            file_sets.append(dep[FuchsiaDeviceTreeSegmentInfo].files)
        if CcInfo in dep:
            context = dep[CcInfo].compilation_context
            includes_sets.append(context.system_includes)
            includes_sets.append(context.quote_includes)
            headers.extend(context.direct_public_headers)

    include_depset = depset(
        transitive = includes_sets,
        order = order,
    )

    files = depset(
        transitive = file_sets,
        order = order,
    )

    # Computer the dts file that we can pass to compiler
    dts_file = source
    if source.extension == "S":
        dts_file = preprocess_file(ctx, source, include_depset, headers, files)

    # Invoke dtc to compiles the dtb
    dtb_filename = dts_file.basename.replace("dts", "dtb")
    output_file = ctx.actions.declare_file(dtb_filename)

    pp_args = ctx.actions.args()
    pp_args.add_all(include_depset, before_each = "-i")
    pp_args.add(source)
    pp_args.add("-o", output_file)
    pp_args.add("-O", "dtb")
    pp_args.add_all(ctx.attr.dtcflags)

    dtc = ctx.toolchains["@rules_fuchsia//fuchsia:devicetree_toolchain_type"].dtc
    ctx.actions.run(
        executable = dtc,
        arguments = [pp_args],
        inputs = [dts_file] + headers + files.to_list(),
        outputs = [output_file],
        mnemonic = "DtbCompile",
    )

    return [
        DefaultInfo(
            files = depset([output_file]),
        ),
    ]

fuchsia_devicetree = rule(
    doc = """Defines a devicetree to be built into a devicetree blob.""",
    implementation = _fuchsia_devicetree_impl,
    toolchains = ["@rules_fuchsia//fuchsia:devicetree_toolchain_type"],
    attrs = {
        "deps": attr.label_list(
            doc = "Other Devicetree fragment targets referenced by this fragment",
            providers = [[FuchsiaDeviceTreeSegmentInfo], [CcInfo]],
            # The dependencies can be cc_library() targets, which need a C++ toolchain
            # definition, even if only their headers are actually used here. A
            # transition to a Fuchsia-compatible build configuration is injected
            # here to ensure that a C++ toolchain is always defined for them.
            cfg = fuchsia_transition,
        ),
        "source": attr.label(
            doc = """Device tree source file (.dts/.dts.S). Source file that
            include C header files should end with the extension `.dts.S` and it
            will be preprocessed by C compiler before invoking the devicetree
            compiler.""",
            allow_single_file = [".dts", ".dts.S"],
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    } | FUCHSIA_DEVICETREE_ATTR,
)

def _fuchsia_devicetree_source_impl(ctx):
    source = ctx.file.source

    # Invoke dtc to decompiles the dtb into a dts
    dts_filename = source.basename.replace("dtb", "dts")
    output_file = ctx.actions.declare_file(dts_filename)

    pp_args = ctx.actions.args()
    pp_args.add("-I", "dtb")
    pp_args.add("-O", "dts")
    pp_args.add("--sort")
    pp_args.add(source)
    pp_args.add("-o", output_file)
    pp_args.add_all(ctx.attr.dtcflags)

    dtc = ctx.toolchains["@rules_fuchsia//fuchsia:devicetree_toolchain_type"].dtc
    ctx.actions.run(
        executable = dtc,
        arguments = [pp_args],
        inputs = [source],
        outputs = [output_file],
        mnemonic = "DtbDecompile",
    )

    return [
        DefaultInfo(
            files = depset([output_file]),
        ),
    ]

fuchsia_devicetree_source = rule(
    doc = """Defines a devicetree blob decompiler. This is mostly used for
    devicetree binary validation.""",
    implementation = _fuchsia_devicetree_source_impl,
    toolchains = ["@rules_fuchsia//fuchsia:devicetree_toolchain_type"],
    attrs = {
        "source": attr.label(
            doc = """Device tree blob file to be decompiled.""",
            allow_single_file = [".dtb"],
        ),
    } | FUCHSIA_DEVICETREE_ATTR,
)
