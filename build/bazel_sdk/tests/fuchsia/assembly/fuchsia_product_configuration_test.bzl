# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@rules_fuchsia//fuchsia_rules_common/assembly:providers.bzl", "FuchsiaProductConfigInfo")
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_product_configuration_test_impl(ctx):
    golden_file = ctx.file.golden_file
    product_config = ctx.attr.product_config[FuchsiaProductConfigInfo]

    if len(ctx.files.product_config) == 1:
        # This is a newly constructed product config with the only file being
        # the directory.
        config_dir = ctx.files.product_config[0]
    else:
        # This is a prebuilt product config.
        config_dir = product_config.directory

    relative_path = "product_configuration.json"
    return [create_validation_script_provider(
        ctx,
        config_dir,
        golden_file,
        relative_path,
        runfiles = ctx.runfiles(files = ctx.files.product_config),
        is_subset = True,
    )]

fuchsia_product_configuration_test = rule(
    doc = """Validate the generated product configuration file.""",
    test = True,
    implementation = _fuchsia_product_configuration_test_impl,
    attrs = {
        "product_config": attr.label(
            doc = "Built Product Config.",
            providers = [FuchsiaProductConfigInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)

def _fuchsia_product_ota_config_test_impl(ctx):
    golden_file = ctx.file.golden_file
    product_config = ctx.attr.product_config[FuchsiaProductConfigInfo]

    if len(ctx.files.product_config) == 1:
        # This is a newly constructed product config with the only file being
        # the directory.
        config_dir = ctx.files.product_config[0]
    else:
        # This is a prebuilt product config.
        config_dir = product_config.directory

    relative_path = ctx.attr.path_in_config
    return [create_validation_script_provider(
        ctx,
        config_dir,
        golden_file,
        relative_path,
        runfiles = ctx.runfiles(files = ctx.files.product_config),
        is_subset = True,
    )]

fuchsia_product_ota_config_test = rule(
    doc = """Validate a generated ota config file from a product config label""",
    test = True,
    implementation = _fuchsia_product_ota_config_test_impl,
    attrs = {
        "product_config": attr.label(
            doc = "Built Product Config.",
            providers = [FuchsiaProductConfigInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Validate that the file with the same name is produced by the product config rule, and matches in contents.",
            allow_single_file = True,
            mandatory = True,
        ),
        "path_in_config": attr.string(
            doc = "The path of the file to validate inside the config",
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
