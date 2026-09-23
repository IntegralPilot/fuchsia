# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for creating an update package."""

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:ffx_tool.bzl", "get_ffx_assembly_args", "get_ffx_assembly_inputs")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    ":providers.bzl",
    "FuchsiaPartitionsConfigInfo",
    "FuchsiaProductImageInfo",
    "FuchsiaUpdatePackageInfo",
)

def _fuchsia_update_package_impl(ctx):
    fuchsia_toolchain = get_fuchsia_sdk_toolchain(ctx)
    partitions_configuration = ctx.attr.partitions_config[FuchsiaPartitionsConfigInfo]
    system_a_out = ctx.attr.main[FuchsiaProductImageInfo].images_out

    out_dir = ctx.actions.declare_directory(ctx.label.name + "_out")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_ffx_isolate_dir")

    inputs = get_ffx_assembly_inputs(fuchsia_toolchain) + [partitions_configuration.files] + ctx.files.main
    outputs = [out_dir, ffx_isolate_dir]

    # Gather all the arguments to pass to ffx.
    ffx_invocation = get_ffx_assembly_args(fuchsia_toolchain) + [
        "--isolate-dir",
        ffx_isolate_dir.path,
        "assembly",
        "create-update",
        "--partitions",
        partitions_configuration.directory,
        "--epoch",
        ctx.attr.update_epoch,
        "--outdir",
        out_dir.path,
        "--system-a",
        system_a_out.path + "/images.json",
    ]

    if ctx.attr.recovery:
        system_r_out = ctx.attr.recovery[FuchsiaProductImageInfo].images_out
        ffx_invocation += [
            "--system-r",
            system_r_out.path + "/images.json",
        ]
        inputs += ctx.files.recovery

    script_lines = [
        "set -e",
        "mkdir -p " + ffx_isolate_dir.path,
        " ".join(ffx_invocation),
    ]
    script = "\n".join(script_lines)

    ctx.actions.run_shell(
        inputs = inputs,
        outputs = outputs,
        command = script,
        mnemonic = "AssemblyCreateUpdate",
        progress_message = "Create update package for %s" % ctx.label.name,
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [
        DefaultInfo(files = depset(direct = outputs + inputs)),
        OutputGroupInfo(
            debug_files = depset([ffx_isolate_dir]),
            all_files = depset([out_dir, ffx_isolate_dir] + inputs),
        ),
        FuchsiaUpdatePackageInfo(
            update_out = out_dir,
        ),
    ]

fuchsia_update_package = rule(
    doc = """Declares a Fuchsia update package.""",
    implementation = _fuchsia_update_package_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaUpdatePackageInfo],
    attrs = {
        "main": attr.label(
            doc = "fuchsia product to put in slot A.",
            providers = [FuchsiaProductImageInfo],
        ),
        "recovery": attr.label(
            doc = "fuchsia product to put in slot R.",
            providers = [FuchsiaProductImageInfo],
        ),
        "partitions_config": attr.label(
            doc = "Partitions config to use.",
            mandatory = True,
        ),
        "update_epoch": attr.string(
            doc = "Epoch needed to create update package.",
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
