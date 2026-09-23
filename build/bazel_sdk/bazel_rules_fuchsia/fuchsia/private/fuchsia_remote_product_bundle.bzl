# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Allows remote product bundle targets to be defined with workflows."""

load(
    "//fuchsia/private/workflows:fuchsia_product_bundle_tasks.bzl",
    "fuchsia_product_bundle_tasks",
    "product_bundles_help_executable",
)
load("//fuchsia/private/workflows:providers.bzl", "FuchsiaProductBundleInfo")

def fuchsia_remote_product_bundle(
        *,
        name,
        transfer_url,
        product_version,
        product_bundle_name = None,
        **kwargs):
    """Describes a product bundle which is not built locally and tasks that can be performed with it.


    The following tasks will be created:
     - name.download: Downloads the product_bundle.
     - name.emu: Starts an emulator with the product_bundle.
     - name.flash: Flashes a device with the product_bundle.

    Args:
        name: The target name.
        transfer_url: The transfer url to fetch the product bundle.
        product_bundle_name: The name of the product bundle.
            Defaults to `name`.
        product_version: The sdk version associated with this product bundle.
        **kwargs: Extra attributes to pass along to the build rule.
    """
    product_bundle_name = product_bundle_name or name

    _fuchsia_remote_product_bundle(
        name = name,
        transfer_url = transfer_url,
        product_bundle_name = product_bundle_name,
        product_version = product_version,
        **kwargs
    )

    fuchsia_product_bundle_tasks(
        name = name + "_tasks",
        product_bundle = name,
        is_remote = True,
        **kwargs
    )

def _fuchsia_remote_product_bundle_impl(ctx):
    return [
        DefaultInfo(
            executable = product_bundles_help_executable(ctx, is_remote = True),
        ),
        FuchsiaProductBundleInfo(
            is_remote = True,
            product_bundle = ctx.attr.transfer_url,
            product_bundle_name = ctx.attr.product_bundle_name,
            product_version = ctx.attr.product_version,
        ),
    ]

_fuchsia_remote_product_bundle = rule(
    implementation = _fuchsia_remote_product_bundle_impl,
    doc = "A rule describing a remote product bundle.",
    attrs = {
        "transfer_url": attr.string(
            doc = "The transfer url to fetch the product bundle.",
            mandatory = True,
        ),
        "product_bundle_name": attr.string(
            doc = "The name of the product bundle.",
            mandatory = True,
        ),
        "product_version": attr.string(
            doc = "The sdk version associated with this product bundle.",
            mandatory = True,
        ),
    },
    executable = True,
)
