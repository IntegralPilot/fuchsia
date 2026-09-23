# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# buildifier: disable=module-docstring
load("@rules_fuchsia//fuchsia/private/assembly:providers.bzl", "FuchsiaVirtualDeviceInfo")
load("//test_utils:json_validator.bzl", "CREATE_VALIDATION_SCRIPT_ATTRS", "create_validation_script_provider")

def _fuchsia_virtual_device_test_impl(ctx):
    virtual_device_file = ctx.attr.virtual_device[FuchsiaVirtualDeviceInfo].config
    golden_file = ctx.file.golden_file
    return [create_validation_script_provider(ctx, virtual_device_file, golden_file)]

fuchsia_virtual_device_test = rule(
    doc = """Validate the generated virtual device config file.""",
    test = True,
    implementation = _fuchsia_virtual_device_test_impl,
    attrs = {
        "virtual_device": attr.label(
            doc = "Built virtual device config.",
            providers = [FuchsiaVirtualDeviceInfo],
            mandatory = True,
        ),
        "golden_file": attr.label(
            doc = "Golden file to match against",
            allow_single_file = True,
            mandatory = True,
        ),
    } | CREATE_VALIDATION_SCRIPT_ATTRS,
)
