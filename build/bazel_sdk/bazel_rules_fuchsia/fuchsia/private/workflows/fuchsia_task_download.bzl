# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Download a product bundle as a task workflow."""

load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":fuchsia_shell_task.bzl", "shell_task_rule")
load(":providers.bzl", "FuchsiaProductBundleInfo")

def get_product_bundle_dir(ctx):
    pb = ctx.attr.product_bundle[FuchsiaProductBundleInfo]
    if pb.is_remote:
        return "/tmp/%s-%s" % (pb.product_bundle_name, pb.product_version)
    else:
        return pb.product_bundle

def _fuchsia_task_download_impl(ctx, make_shell_task):
    pb = ctx.attr.product_bundle[FuchsiaProductBundleInfo]
    if not pb.is_remote:
        fail("fuchsia_task_download can only be used for remote product bundles.")

    output_dir = get_product_bundle_dir(ctx)
    sdk = get_fuchsia_sdk_toolchain(ctx)
    return make_shell_task(
        command = [
            "echo",
            "Ensuring product bundle is downloaded to: %s" % output_dir,
            "&&",
            "test",
            "-d",
            output_dir,
            "||",
            sdk.ffx,
            "product",
            "download",
            pb.product_bundle,
            output_dir,
        ],
    )

_fuchsia_task_download, _fuchsia_task_download_for_test, fuchsia_task_download = shell_task_rule(
    doc = """Downloads a product bundle.""",
    implementation = _fuchsia_task_download_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "product_bundle": attr.label(
            doc = "The product bundle to download",
            providers = [FuchsiaProductBundleInfo],
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
