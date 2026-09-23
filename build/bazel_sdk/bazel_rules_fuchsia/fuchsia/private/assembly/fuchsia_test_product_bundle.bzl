# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Define a test-only product bundle"""

load(
    "//fuchsia/private/assembly:fuchsia_product.bzl",
    "fuchsia_product",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_bundle.bzl",
    "fuchsia_product_bundle",
)
load(
    "//fuchsia/private/assembly:fuchsia_product_configuration.bzl",
    "fuchsia_product_configuration",
)

def fuchsia_test_product_bundle(
        name,
        board_config,
        product_config_json,
        cache_packages = [],
        virtual_devices = []):
    """A macro for defining a test-only product bundle.

    This macro simplifies the creation of a product bundle for testing purposes
    by wrapping the fuchsia_product_bundle, fuchsia_product, and
    fuchsia_product_configuration rules into a single macro.

    Args:
        name: The name of the product bundle.
        board_config: The board configuration target.
        product_config_json: A dictionary for the product configuration.
        cache_packages: A list of fuchsia_package targets to cache.
        virtual_devices: The fuchsia_virtual_device()s to include for running on emulators.
    """
    product_config_name = name + ".product_config"
    fuchsia_product_configuration(
        name = product_config_name,
        product_config_json = product_config_json,
        cache_packages = cache_packages,
    )

    product_name = name + ".product"
    fuchsia_product(
        name = product_name,
        board_config = board_config,
        platform_artifacts = "//build/bazel/assembly/assembly_input_bundles:platform_eng",
        product_config = ":" + product_config_name,
    )

    fuchsia_product_bundle(
        name = name,
        main = ":" + product_name,
        product_bundle_name = name,
        virtual_devices = virtual_devices,
    )
