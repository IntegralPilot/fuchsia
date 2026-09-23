# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for defining a Fuchsia toolchain."""

load("@rules_fuchsia//fuchsia/private:providers.bzl", "FuchsiaComponentManifestShardCollectionInfo")

def _fuchsia_toolchain_info_impl(ctx):
    return [platform_common.ToolchainInfo(
        name = ctx.label.name,
        aemu_runfiles = ctx.runfiles(ctx.files.aemu_runfiles),
        assembly_config = ctx.executable.assembly_config,
        blobfs = ctx.executable.blobfs,
        blobfs_manifest = ctx.file.blobfs_manifest,
        bindc = ctx.executable.bindc or None,
        configc = ctx.executable.configc,
        cmc = ctx.executable.cmc,
        cmc_manifest = ctx.file.cmc_manifest,
        cmc_includes = ctx.attr.cmc_includes or None,
        elf_test_runner_shard = ctx.attr.elf_test_runner_shard,
        ffx = ctx.executable.ffx,
        ffx_assembly = ctx.executable.ffx_assembly or None,
        ffx_assembly_fho_meta = ctx.file.ffx_assembly_fho_meta or None,
        ffx_assembly_manifest = ctx.file.ffx_assembly_manifest or None,
        ffx_driver = ctx.executable.ffx_driver or None,
        ffx_driver_fho_meta = ctx.file.ffx_driver_fho_meta or None,
        ffx_driver_manifest = ctx.file.ffx_driver_manifest or None,
        ffx_package = ctx.executable.ffx_package or None,
        ffx_package_fho_meta = ctx.file.ffx_package_fho_meta or None,
        ffx_package_manifest = ctx.file.ffx_package_manifest or None,
        ffx_product = ctx.executable.ffx_product or None,
        ffx_product_fho_meta = ctx.file.ffx_product_fho_meta or None,
        ffx_product_manifest = ctx.file.ffx_product_manifest or None,
        ffx_scrutiny = ctx.executable.ffx_scrutiny or None,
        ffx_scrutiny_fho_meta = ctx.file.ffx_scrutiny_fho_meta or None,
        ffx_scrutiny_manifest = ctx.file.ffx_scrutiny_manifest or None,
        ffx_test = ctx.executable.ffx_test or None,
        ffx_test_fho_meta = ctx.file.ffx_test_fho_meta or None,
        ffx_test_manifest = ctx.file.ffx_test_manifest or None,
        fidlc = ctx.executable.fidlc,
        fidl_format = ctx.executable.fidl_format,
        fidlgen_hlcpp = ctx.executable.fidlgen_hlcpp,
        fidlgen_cpp = ctx.executable.fidlgen_cpp,
        funnel = ctx.executable.funnel,
        fvm = ctx.executable.fvm,
        fvm_manifest = ctx.file.fvm_manifest,
        gen_android_starnix_container = ctx.executable.gen_android_starnix_container,
        gtest_runner_shard = ctx.attr.gtest_runner_shard,
        merkleroot = ctx.executable.merkleroot,
        minfs = ctx.executable.minfs,
        minfs_manifest = ctx.file.minfs_manifest,
        symbolizer = ctx.executable.symbolizer,
        symbolizer_manifest = ctx.file.symbolizer_manifest,
        symbol_index_config = ctx.runfiles(ctx.files.symbol_index_config),
        zbi = ctx.executable.zbi,
        zbi_manifest = ctx.file.zbi_manifest,
        exec_cpu = ctx.attr.exec_cpu,
        sdk_id = ctx.attr.sdk_id,
        sdk_manifest = ctx.file.sdk_manifest,
    )]

fuchsia_toolchain_info = rule(
    implementation = _fuchsia_toolchain_info_impl,
    doc = """
Fuchsia toolchain info rule, to be passed to the native `toolchain` rule.

It provides information about tools in the Fuchsia toolchain, primarily those
included in the Fuchsia IDK.
""",
    attrs = {
        "aemu_runfiles": attr.label(
            doc = "emulator runfiles",
            mandatory = True,
            cfg = "exec",
        ),
        "assembly_config": attr.label(
            doc = "assembly_config tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "blobfs": attr.label(
            doc = "blobfs tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "blobfs_manifest": attr.label(
            doc = "blobfs tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "bindc": attr.label(
            doc = "bindc tool executable.",
            mandatory = False,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "configc": attr.label(
            doc = "configc tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "cmc": attr.label(
            doc = "cmc tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "cmc_manifest": attr.label(
            doc = "cmc tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "cmc_includes": attr.label(
            doc = "The collection of cml files to include in the cmc invocation",
            providers = [[FuchsiaComponentManifestShardCollectionInfo]],
        ),
        "elf_test_runner_shard": attr.string(
            doc = "The path to the elf test runner's cml shard.",
            mandatory = True,
        ),
        "ffx": attr.label(
            doc = "ffx tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_assembly": attr.label(
            doc = "ffx-assembly tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_assembly_fho_meta": attr.label(
            doc = "ffx-assembly tool metadata.",
            allow_single_file = True,
        ),
        "ffx_assembly_manifest": attr.label(
            doc = "ffx-assembly tool metadata.",
            allow_single_file = True,
        ),
        "ffx_driver": attr.label(
            doc = "ffx-driver tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_driver_fho_meta": attr.label(
            doc = "ffx-driver tool metadata.",
            allow_single_file = True,
        ),
        "ffx_driver_manifest": attr.label(
            doc = "ffx-driver tool metadata.",
            allow_single_file = True,
        ),
        "ffx_package": attr.label(
            doc = "ffx-package tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_package_fho_meta": attr.label(
            doc = "ffx-package tool metadata.",
            allow_single_file = True,
        ),
        "ffx_package_manifest": attr.label(
            doc = "ffx-package tool metadata.",
            allow_single_file = True,
        ),
        "ffx_product": attr.label(
            doc = "ffx-product tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_product_fho_meta": attr.label(
            doc = "ffx-product tool metadata.",
            allow_single_file = True,
        ),
        "ffx_product_manifest": attr.label(
            doc = "ffx-product tool metadata.",
            allow_single_file = True,
        ),
        "ffx_scrutiny": attr.label(
            doc = "ffx-scrutiny tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_scrutiny_fho_meta": attr.label(
            doc = "ffx-scrutiny tool metadata.",
            allow_single_file = True,
        ),
        "ffx_scrutiny_manifest": attr.label(
            doc = "ffx-scrutiny tool metadata.",
            allow_single_file = True,
        ),
        "ffx_test": attr.label(
            doc = "ffx-test tool executable.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "ffx_test_fho_meta": attr.label(
            doc = "ffx-test tool metadata.",
            allow_single_file = True,
        ),
        "ffx_test_manifest": attr.label(
            doc = "ffx-test tool metadata.",
            allow_single_file = True,
        ),
        "fidlc": attr.label(
            doc = "fidlc tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "fidl_format": attr.label(
            doc = "fidl-format tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "fidlgen_hlcpp": attr.label(
            doc = "fidlgen_hlcpp tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "fidlgen_cpp": attr.label(
            doc = "fidlgen_cpp tool executable.",
            mandatory = False,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "funnel": attr.label(
            doc = "funnel tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "fvm": attr.label(
            doc = "fvm tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "fvm_manifest": attr.label(
            doc = "fvm tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "gen_android_starnix_container": attr.label(
            doc = "gen_android_starnix_container tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "gtest_runner_shard": attr.string(
            doc = "The path to the gtest runner's cml shard.",
            mandatory = True,
        ),
        "merkleroot": attr.label(
            doc = "merkleroot tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "minfs": attr.label(
            doc = "minfs tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "minfs_manifest": attr.label(
            doc = "minfs tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "symbolizer": attr.label(
            doc = "symbolizer tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "symbolizer_manifest": attr.label(
            doc = "symbolizer tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "symbol_index_config": attr.label(
            doc = "symbol-index config files, required by symbolizer.",
            mandatory = True,
            cfg = "exec",
        ),
        "zbi": attr.label(
            doc = "zbi tool executable.",
            mandatory = True,
            cfg = "exec",
            executable = True,
            allow_single_file = True,
        ),
        "zbi_manifest": attr.label(
            doc = "zbi tool's manifest, required by ffx.",
            mandatory = True,
            cfg = "exec",
            allow_single_file = True,
        ),
        "exec_cpu": attr.string(
            doc = "The exec cpu configuration.",
            mandatory = True,
            values = ["x64", "arm64"],
        ),
        "sdk_id": attr.string(
            doc = "The identifier for this sdk toolchain.",
            mandatory = True,
        ),
        "sdk_manifest": attr.label(
            doc = "Top-level SDK manifest location.",
            mandatory = True,
            allow_single_file = True,
        ),
    },
    provides = [platform_common.ToolchainInfo],
)
