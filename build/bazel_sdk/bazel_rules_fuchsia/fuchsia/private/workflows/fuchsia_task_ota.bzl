# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""OTA device using product bundle as a task workflow."""

load(":fuchsia_task_download.bzl", "get_product_bundle_dir")
load(":fuchsia_task_ffx.bzl", "ffx_task_rule")
load(":providers.bzl", "FuchsiaProductBundleInfo")

def _fuchsia_task_ota_impl(ctx, _make_ffx_task):
    pb_path = get_product_bundle_dir(ctx)
    return _make_ffx_task(
        prepend_args = [
            "target",
            "update",
            "check-now",
            "--monitor",
            "--product-bundle",
            pb_path,
        ],
    )

_fuchsia_task_ota, _fuchsia_task_ota_for_test, fuchsia_task_ota = ffx_task_rule(
    doc = """OTA device using product bundle.""",
    implementation = _fuchsia_task_ota_impl,
    attrs = {
        "product_bundle": attr.label(
            doc = "Product bundle that is needed to OTA the device",
            providers = [FuchsiaProductBundleInfo],
            mandatory = True,
        ),
    },
)
