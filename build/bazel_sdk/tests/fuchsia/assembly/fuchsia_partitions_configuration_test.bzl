# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@rules_fuchsia//fuchsia/private/assembly:providers.bzl", "FuchsiaPartitionsConfigInfo")
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_partitions_configuration_test_impl(ctx):
    golden_file = ctx.file.golden_file
    partitions_config = ctx.attr.partitions_config[FuchsiaPartitionsConfigInfo]
    if len(ctx.files.partitions_config) == 1:
        # This is a newly constructed partitions config with the only file being
        # the directory.
        config_dir = ctx.files.partitions_config[0]
    else:
        # This is a prebuilt partitions config.
        config_dir = partitions_config.directory

    relative_path = "partitions_config.json"
    return [create_validation_script_provider(
        ctx,
        config_dir,
        golden_file,
        relative_path,
        runfiles = ctx.runfiles(files = ctx.files.partitions_config),
    )]

fuchsia_partitions_configuration_test = rule(
    doc = """Validate the generated partitions configuration file.""",
    test = True,
    implementation = _fuchsia_partitions_configuration_test_impl,
    attrs = {
        "partitions_config": attr.label(
            doc = "Built partitions Config.",
            providers = [FuchsiaPartitionsConfigInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
