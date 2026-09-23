# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for defining assembly product input bundles."""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("@fuchsia_rules_common//assembly:providers.bzl", "FuchsiaProductInputBundleInfo")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":utils.bzl", "select_root_dir_with_file")

def _fuchsia_product_input_bundle_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    creation_args = []
    creation_inputs = []
    for dep in ctx.attr.base_packages:
        creation_args.extend(
            [
                "--base-packages",
                dep[FuchsiaPackageInfo].package_manifest.path,
            ],
        )
        creation_inputs += dep[FuchsiaPackageInfo].files
    for dep in ctx.attr.cache_packages:
        creation_args.extend(
            [
                "--cache-packages",
                dep[FuchsiaPackageInfo].package_manifest.path,
            ],
        )
        creation_inputs += dep[FuchsiaPackageInfo].files
    for dep in ctx.attr.flexible_packages:
        creation_args.extend(
            [
                "--flexible-packages",
                dep[FuchsiaPackageInfo].package_manifest.path,
            ],
        )
        creation_inputs += dep[FuchsiaPackageInfo].files
    for dep in ctx.attr.packages_for_product_config:
        creation_args.extend(
            [
                "--packages-for-product-config",
                dep[FuchsiaPackageInfo].package_manifest.path,
            ],
        )
        creation_inputs += dep[FuchsiaPackageInfo].files

    # Ensure that exactly one of "version" and "version_file" is set.
    # Note: an empty string "" is acceptable: non-official builds may not
    # have access to a version number. The config generator tool will convert
    # the empty string to "unversioned".
    if ((ctx.attr.version != "__unset") == (ctx.attr.version_file != None)):
        fail("Exactly one of \"version\" or \"version_file\" must be set.")

    if ctx.attr.version != "__unset":
        creation_args.extend(["--version", ctx.attr.version])
    elif ctx.file.version_file:
        creation_args.extend(["--version-file", ctx.file.version_file.path])
        creation_inputs.append(ctx.file.version_file)

    if ctx.attr.repo:
        repo = ctx.attr.repo
    else:
        repo = ctx.attr._release_repository_flag[BuildSettingInfo].value

    # TODO(https://b.corp.google.com/issues/416239346): Make "repo" field
    # required.
    if repo:
        creation_args += ["--repo", repo]

    # Create Product Input Bundle
    product_input_bundle_dir = ctx.actions.declare_directory(ctx.label.name)
    args = [
        "generate",
        "product-input-bundle",
        "--name",
        ctx.label.name,
        "--output",
        product_input_bundle_dir.path,
    ] + creation_args
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = creation_inputs,
        outputs = [product_input_bundle_dir],
        progress_message = "Creating product input bundle for %s" % ctx.label.name,
        mnemonic = "CreatePIB",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(
            files = depset([product_input_bundle_dir]),
        ),
        FuchsiaProductInputBundleInfo(
            directory = product_input_bundle_dir.path,
            build_id_dirs = [],
        ),
    ]

fuchsia_product_input_bundle = rule(
    doc = """Generates a product input bundle.""",
    implementation = _fuchsia_product_input_bundle_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "base_packages": attr.label_list(
            doc = "Base packages to include in the product.",
            providers = [FuchsiaPackageInfo],
            default = [],
        ),
        "cache_packages": attr.label_list(
            doc = "Cache packages to include in the product.",
            providers = [FuchsiaPackageInfo],
            default = [],
        ),
        "flexible_packages": attr.label_list(
            doc = "Flexible packages to include in the product.",
            providers = [FuchsiaPackageInfo],
            default = [],
        ),
        "packages_for_product_config": attr.label_list(
            doc = "Packages that will only be included if they are referenced by the product config.",
            providers = [FuchsiaPackageInfo],
            default = [],
        ),
        "version": attr.string(
            doc = "Release version string",
            default = "__unset",
        ),
        "version_file": attr.label(
            doc = "Path to a file containing the current release version.",
            allow_single_file = True,
        ),
        "repo": attr.string(
            doc = "Name of the release repository. Overrides _release_repository_flag when set.",
        ),
        "_release_repository_flag": attr.label(
            doc = "String flag used to set the name of the release repository.",
            default = "@rules_fuchsia//fuchsia/flags:fuchsia_release_repository",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def _fuchsia_prebuilt_product_input_bundle_impl(ctx):
    directory = select_root_dir_with_file(ctx.files.files, "product_input_bundle.json")
    return [
        DefaultInfo(files = depset(ctx.files.files)),
        FuchsiaProductInputBundleInfo(
            directory = directory,
            build_id_dirs = [],
        ),
    ]

fuchsia_prebuilt_product_input_bundle = rule(
    doc = """Defines a Product Input Bundle based on preexisting PIB files.""",
    implementation = _fuchsia_prebuilt_product_input_bundle_impl,
    attrs = {
        "files": attr.label(
            doc = "All files belonging to the Product Input Bundle",
            mandatory = True,
        ),
    },
)
