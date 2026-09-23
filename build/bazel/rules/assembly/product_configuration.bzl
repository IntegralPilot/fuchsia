# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_rules_common//assembly:json_utils.bzl", "extract_labels")
load(
    "@fuchsia_rules_common//assembly:product_configuration.bzl",
    "COMMON_PRODUCT_ASSEMBLY_ATTRIBUTES",
    "common_product_configuration_impl",
)
load("@fuchsia_rules_common//assembly:providers.bzl", "FuchsiaProductConfigInfo")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")

def _product_configuration_impl(ctx):
    assembly_config_binary = ctx.attr._assembly_config[DefaultInfo].files.to_list()[0]

    return common_product_configuration_impl(
        ctx,
        assembly_config_binary,
        bootfs_files_package = ctx.attr.bootfs_files_package,
    )

_product_configuration = rule(
    doc = """Generates a product configuration file for Fuchsia platform testing (in-tree) only.""",
    implementation = _product_configuration_impl,
    provides = [FuchsiaProductConfigInfo],
    # Use the common attributes for product assembly, but add the following in-tree-only attributes
    # and the file for the assembly_config binary from the GN portion of the build.
    attrs = COMMON_PRODUCT_ASSEMBLY_ATTRIBUTES | {
        "bootfs_files_package": attr.label(
            doc = "A package of files to include in the bootfs of the zbi.",
            providers = [FuchsiaPackageInfo],
            default = None,
        ),
        "enable_example_aib": attr.bool(
            doc = "Optionally enable example AIBs for testing.",
            default = False,
        ),
        "_assembly_config": attr.label(
            default = "@gn_targets//toolchain_host_x64/build/assembly/tools/assembly_config:assembly_config",
        ),
    },
)

def product_configuration(
        *,
        name,
        product_config_json = None,
        bootfs_packages = None,
        bootfs_files_package = None,
        base_packages = None,
        cache_packages = None,
        base_driver_packages = None,
        ota_configuration = None,
        starnix_containers = [],
        enable_example_aib = False,
        **kwargs):
    """Generates a product configuration file for Fuchsia platform testing (in-tree) only."""
    json_config = product_config_json if product_config_json else {}
    _product_configuration(
        name = name,
        product_config = json.encode_indent(json_config, indent = "    "),
        product_config_labels = extract_labels(json_config),
        bootfs_packages = bootfs_packages,
        bootfs_files_package = bootfs_files_package,
        base_packages = base_packages,
        cache_packages = cache_packages,
        base_driver_packages = base_driver_packages,
        ota_configuration = ota_configuration,
        starnix_containers = starnix_containers,
        enable_example_aib = enable_example_aib,
        **kwargs
    )
