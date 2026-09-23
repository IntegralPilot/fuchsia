# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for running size checker on given image."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:ffx_tool.bzl", "get_ffx_assembly_args", "get_ffx_assembly_inputs")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":providers.bzl", "FuchsiaProductImageInfo", "FuchsiaSizeCheckerInfo")

# Command for running ffx assembly size-check product.
_SIZE_CHECKER_RUNNER_SH = """
set -e

mkdir -p $FFX_ISOLATE_DIR
{ffx_assembly_args} \
    "--isolate-dir" \
    $FFX_ISOLATE_DIR \
    assembly \
    size-check \
    product \
    --assembly-manifest $IMAGES_PATH \
    --size-breakdown-output $SIZE_FILE \
    --visualization-dir $VISUALIZATION_DIR \
    {extra_args}
"""

def _fuchsia_product_size_check_impl(ctx):
    images_out = ctx.attr.product_image[FuchsiaProductImageInfo].images_out
    fuchsia_toolchain = get_fuchsia_sdk_toolchain(ctx)

    size_file = ctx.actions.declare_file(ctx.label.name + "_size_breakdown.txt")
    size_report_product_file = ctx.actions.declare_file(ctx.label.name + "_size_report_product.json")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_ffx_isolate_dir")

    visualization_outputs = [
        ctx.actions.declare_file(paths.join(ctx.label.name + "_visualization", p))
        for p in (
            "data.js",
            "index.html",
            "D3BlobTreeMap.js",
            paths.join("d3_v3", "d3.js"),
            paths.join("d3_v3", "LICENSE"),
        )
    ]
    visualization_dir_path = visualization_outputs[0].dirname

    final_outputs = [size_file, size_report_product_file] + visualization_outputs

    _env = {
        "FFX_ISOLATE_DIR": ffx_isolate_dir.path,
        "IMAGES_PATH": images_out.path + "/assembled_system.json",
        "SIZE_FILE": size_file.path,
        "VISUALIZATION_DIR": visualization_dir_path,
    }
    extra_args = []

    # If blobfs_creep_limit and platform_resources_budget are empty return an empty size report file
    # if only one of them is missing exit with an error otherwise make a size report file.

    if ctx.attr.blobfs_creep_limit == None and ctx.attr.platform_resources_budget == None:
        ctx.actions.write(output = size_report_product_file, content = "{}")

    elif ctx.attr.blobfs_creep_limit == None:
        fail("Attribute blobfs_creep_limit is missing for %s." % ctx.label.name)

    elif ctx.attr.platform_resources_budget == None:
        fail("Attribute platform_resources_budget is missing for %s." % ctx.label.name)

    else:
        extra_args += [
            "--gerrit-output",
            size_report_product_file.path,
            "--blobfs-creep-budget",
            str(ctx.attr.blobfs_creep_limit),
            "--platform-resources-budget",
            str(ctx.attr.platform_resources_budget),
        ]
    ctx.actions.run_shell(
        inputs = ctx.files.product_image + get_ffx_assembly_inputs(fuchsia_toolchain),
        outputs = final_outputs + [ffx_isolate_dir],
        mnemonic = "FuchsiaProductSizeCheck",
        command = _SIZE_CHECKER_RUNNER_SH.format(
            ffx_assembly_args = " ".join(get_ffx_assembly_args(fuchsia_toolchain)),
            extra_args = " ".join(extra_args),
        ),
        env = _env,
        progress_message = "Size checking for %s" % ctx.label.name,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset(direct = final_outputs)),
        FuchsiaSizeCheckerInfo(
            size_report = size_report_product_file,
        ),
    ]

fuchsia_product_size_check = rule(
    doc = """Create a size summary of an image.""",
    implementation = _fuchsia_product_size_check_impl,
    provides = [FuchsiaSizeCheckerInfo],
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "product_image": attr.label(
            doc = "fuchsia_product target to check the size of.",
            providers = [FuchsiaProductImageInfo],
            mandatory = True,
        ),
        "blobfs_creep_limit": attr.int(
            doc = "Creep limit for Blobfs, which defines how much blobfs can increase without warning.",
        ),
        "platform_resources_budget": attr.int(
            doc = """Space allocated for shared platform resources.
            These are typically shared libraries provided by the Fuchsia SDK
            that can be included in many different components. It can be helpful
            to isolate these resources into a separate budget to enforce a
            specific number of unique copies, and to have a distinct creep
            budget.""",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
