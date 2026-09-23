# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@rules_fuchsia//fuchsia/private/assembly:providers.bzl", "FuchsiaBoardConfigInfo")
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_board_configuration_test_impl(ctx):
    golden_file = ctx.file.golden_file
    board_config = ctx.attr.board_config[FuchsiaBoardConfigInfo]
    if len(ctx.files.board_config) == 1:
        # This is a newly constructed board config with the only file being
        # the directory.
        config_dir = ctx.files.board_config[0]
    else:
        # This is a prebuilt board config.
        config_dir = board_config.directory

    relative_path = "board_configuration.json"
    return [create_validation_script_provider(
        ctx,
        config_dir,
        golden_file,
        relative_path,
        runfiles = ctx.runfiles(files = ctx.files.board_config),
        is_subset = True,
    )]

fuchsia_board_configuration_test = rule(
    doc = """Validate the generated board configuration file.""",
    test = True,
    implementation = _fuchsia_board_configuration_test_impl,
    attrs = {
        "board_config": attr.label(
            doc = "Built Board Config.",
            providers = [FuchsiaBoardConfigInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)

def _fuchsia_hybrid_board_configuration_test_impl(ctx):
    golden_file = ctx.file.golden_bib
    board_config = ctx.attr.hybrid_board_config[FuchsiaBoardConfigInfo]
    if len(ctx.files.hybrid_board_config) == 1:
        # This is a newly constructed board config with the only file being
        # the directory.
        config_dir = ctx.files.hybrid_board_config[0]
    else:
        # This is a prebuilt board config.
        config_dir = board_config.directory

    relative_path = ctx.attr.relative_path_of_board_input_bundle + "/board_input_bundle.json"
    return [create_validation_script_provider(
        ctx,
        config_dir,
        golden_file,
        relative_path,
        runfiles = ctx.runfiles(files = ctx.files.hybrid_board_config),
        is_subset = True,
    )]

fuchsia_hybrid_board_configuration_test = rule(
    doc = """Validate the hybrid board has replaced BIB. Note this test assumes there is only one BIB to verify.""",
    test = True,
    implementation = _fuchsia_hybrid_board_configuration_test_impl,
    attrs = {
        "hybrid_board_config": attr.label(
            doc = "Built hybrid board config target",
            providers = [FuchsiaBoardConfigInfo],
            mandatory = True,
        ),
        "golden_bib": attr.label(
            doc = "Golden BIB file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
        "relative_path_of_board_input_bundle": attr.string(
            doc = "The path inside hyrid_board_config of the board input bundle to check",
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
