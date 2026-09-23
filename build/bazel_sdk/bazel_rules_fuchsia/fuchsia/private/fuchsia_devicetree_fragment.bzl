# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines a devicetree source file that can be included by other devicetree files."""

load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")
load(":fuchsia_transition.bzl", "fuchsia_transition")
load(":providers.bzl", "FuchsiaDeviceTreeSegmentInfo")
load(":utils.bzl", "PREPROCESS_FILE_ATTRS", "preprocess_file")

def _fuchsia_devicetree_fragment_impl(ctx):
    source = ctx.file.source
    output = source
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

    if source.extension == "S":
        output = preprocess_file(ctx, source, include_depset, headers, files)

    return [
        DefaultInfo(
            files = depset([output]),
        ),
        FuchsiaDeviceTreeSegmentInfo(
            includes = depset(
                [output.dirname],
                transitive = [include_depset],
                order = order,
            ),
            files = depset(
                [output],
                transitive = [files],
                order = order,
            ),
        ),
    ]

fuchsia_devicetree_fragment = rule(
    doc = """Defines a devicetree source file that can be included by other
    devicetree files.""",
    implementation = _fuchsia_devicetree_fragment_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Other Devicetree fragment targets referenced by this fragment",
            providers = [[FuchsiaDeviceTreeSegmentInfo], [CcInfo]],
            # A transition to a Fuchsia-compatible build configuration to ensure
            # that a C++ toolchain is always defined for the dependencies, which
            # can be cc_library() target that require one, even though only
            # their headers are used in practice.
            cfg = fuchsia_transition,
        ),
        "source": attr.label(
            doc = """Device tree source include file (.dtsi/.dtsi.S). Source
            files that include C header files should end with the extension
            `.dtsi.S` and it will be preprocessed by C compiler.""",
            allow_single_file = [".dtsi", ".dtsi.S"],
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    } | PREPROCESS_FILE_ATTRS,
)
