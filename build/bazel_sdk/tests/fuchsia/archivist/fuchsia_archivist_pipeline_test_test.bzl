# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_archivist_pipeline_test_test_impl(ctx):
    generated_cml_file = ctx.file.archivist_pipeline_test
    golden_file = ctx.file.golden_file
    return [create_validation_script_provider(ctx, generated_cml_file, golden_file)]

fuchsia_archivist_pipeline_test_test = rule(
    doc = """Validate the generated archivist pipeline test cml file.""",
    test = True,
    implementation = _fuchsia_archivist_pipeline_test_test_impl,
    attrs = {
        "archivist_pipeline_test": attr.label(
            doc = "Built archivist pipeline test cml.",
            allow_single_file = True,
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
