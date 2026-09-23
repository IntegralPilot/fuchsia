# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@rules_fuchsia//fuchsia/private/assembly:providers.bzl", "FuchsiaBoardInputBundleInfo")
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_board_input_bundle_test_impl(ctx):
    board_input_bundle = ctx.attr.board_input_bundle[FuchsiaBoardInputBundleInfo]
    golden_file = ctx.file.golden_file

    if len(ctx.files.board_input_bundle) == 1:
        # This is a newly constructed board input bundle with the only file
        # being the directory.
        board_input_bundle_dir = ctx.files.board_input_bundle[0]
    else:
        # This is a prebuilt board input bundle.
        board_input_bundle_dir = board_input_bundle.directory

    return [
        create_validation_script_provider(
            ctx,
            board_input_bundle_dir,
            golden_file,
            relative_path = "board_input_bundle.json",
            runfiles = ctx.runfiles(files = ctx.files.board_input_bundle),
        ),
    ]

fuchsia_board_input_bundle_test = rule(
    doc = """Validate the generated board input bundle.""",
    test = True,
    implementation = _fuchsia_board_input_bundle_test_impl,
    attrs = {
        "board_input_bundle": attr.label(
            doc = "Built board input bundle.",
            providers = [FuchsiaBoardInputBundleInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
