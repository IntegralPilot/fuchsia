# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for declaring a Fuchsia product configuration."""

# buildifier: disable=module-docstring
load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("@fuchsia_rules_common//assembly:json_utils.bzl", "extract_labels")
load(
    "@fuchsia_rules_common//assembly:product_configuration.bzl",
    "BUILD_TYPES",
    "COMMON_PRODUCT_ASSEMBLY_ATTRIBUTES",
    "common_product_configuration_impl",
)
load(
    "@fuchsia_rules_common//assembly:providers.bzl",
    "FuchsiaProductConfigInfo",
    "FuchsiaProductInputBundleInfo",
)
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load(
    "//fuchsia/private:fuchsia_toolchains.bzl",
    "FUCHSIA_TOOLCHAIN_DEFINITION",
    "get_fuchsia_sdk_toolchain",
)
load(":utils.bzl", "select_root_dir_with_file")

# Define input device types option
INPUT_DEVICE_TYPE = struct(
    BUTTON = "button",
    KEYBOARD = "keyboard",
    LIGHT_SENSOR = "lightsensor",
    MOUSE = "mouse",
    TOUCHSCREEN = "touchscreen",
)

def _fuchsia_product_configuration_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    if ctx.attr.repo:
        repo_name = ctx.attr.repo
    else:
        repo_name = ctx.attr._release_repository_flag[BuildSettingInfo].value

    return common_product_configuration_impl(ctx, sdk.assembly_config, repo_name = repo_name)

def _fuchsia_prebuilt_product_configuration_impl(ctx):
    directory = select_root_dir_with_file(ctx.files.files, "product_configuration.json")
    return [
        DefaultInfo(files = depset(ctx.files.files)),
        FuchsiaProductConfigInfo(
            directory = directory,
            build_type = ctx.attr.build_type,
            build_id_dirs = [],
        ),
    ]

_fuchsia_prebuilt_product_configuration = rule(
    doc = "Use a prebuilt product configuration directory for hybrid assembly.",
    implementation = _fuchsia_prebuilt_product_configuration_impl,
    attrs = {
        "files": attr.label_list(
            doc = "All files referenced by the product config. This should be the entire contents of the product input artifacts directory.",
            mandatory = True,
        ),
        "build_type": attr.string(
            doc = "Build type of the product config. Must match the prebuilts.",
            mandatory = True,
            values = [BUILD_TYPES.ENG, BUILD_TYPES.USER, BUILD_TYPES.USER_DEBUG],
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

_fuchsia_product_configuration = rule(
    doc = """Generates a product configuration file.""",
    implementation = _fuchsia_product_configuration_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    # Use the common product assembly attributes from @fuchsia_rules_common
    attrs = COMMON_PRODUCT_ASSEMBLY_ATTRIBUTES | {
        "_release_repository_flag": attr.label(
            doc = "String flag used to set the name of the release repository.",
            default = "@rules_fuchsia//fuchsia/flags:fuchsia_release_repository",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def fuchsia_prebuilt_product_configuration(
        name,
        product_config_dir,
        build_type,
        # TODO(https://fxbug.dev/427811316): Flip this to False when all
        # products set it to True when necessary.
        allow_empty = True,
        **kwargs):
    _all_files_target = "{}_all_files".format(name)
    native.filegroup(
        name = _all_files_target,
        srcs = native.glob(["{}/**/*".format(product_config_dir)], allow_empty = allow_empty),
    )

    _fuchsia_prebuilt_product_configuration(
        name = name,
        files = [":{}".format(_all_files_target)],
        build_type = build_type,
        **kwargs
    )

def fuchsia_product_configuration(
        name,
        product_config_json = None,
        bootfs_packages = None,
        base_packages = None,
        cache_packages = None,
        base_driver_packages = None,
        ota_configuration = None,
        starnix_containers = [],
        **kwargs):
    """A new implementation of fuchsia_product_configuration that takes raw a json config.

    Args:
        name: Name of the rule.
        product_config_json: product assembly json config, as a starlark dictionary.
            Format of this JSON config can be found in this Rust definitions:
               //src/lib/assembly/config_schema/src/assembly_config.rs

            Key values that take file paths should be declared as a string with
            the label path wrapped via "LABEL(" prefix and ")" suffix. For
            example:
            ```
            {
                "platform": {
                    "some_file": "LABEL(//path/to/file)",
                },
            },
            ```

            All assembly json inputs are supported, except for product.packages
            and product.base_drivers, which must be
            specified through the following args.

            TODO(https://fxbug.dev/42073826): Point to document instead of Rust definition
        bootfs_packages: Fuchsia packages to be included in bootfs.
        base_packages: Fuchsia packages to be included in base.
        cache_packages: Fuchsia packages to be included in cache.
        base_driver_packages: Base driver packages to include in product.
        ota_configuration: OTA configuration to use with the product.
        starnix_containers: List of Starnix containers.
        **kwargs: Common bazel rule args passed through to the implementation rule.
    """

    json_config = product_config_json
    if not product_config_json:
        json_config = {}
    if type(json_config) != "dict":
        fail("expecting a dictionary")

    _fuchsia_product_configuration(
        name = name,
        product_config = json.encode_indent(json_config, indent = "    "),
        product_config_labels = extract_labels(json_config),
        bootfs_packages = bootfs_packages,
        base_packages = base_packages,
        cache_packages = cache_packages,
        base_driver_packages = base_driver_packages,
        ota_configuration = ota_configuration,
        starnix_containers = starnix_containers,
        **kwargs
    )

def _fuchsia_hybrid_product_configuration_impl(ctx):
    replace_packages = []
    for label in ctx.attr.packages:
        replace_packages.append(label[FuchsiaPackageInfo].package_manifest.path)

    product_config_dir = ctx.actions.declare_directory(ctx.label.name)
    product_config = ctx.attr.product_configuration[FuchsiaProductConfigInfo]
    args = [
        "generate",
        "hybrid-product",
        "--input",
        product_config.directory,
        "--output",
        product_config_dir.path,
    ]
    for package_manifest in replace_packages:
        args += ["--replace-package", package_manifest]

    for pib in ctx.attr.product_input_bundles:
        directory = pib[FuchsiaProductInputBundleInfo].directory
        args += ["--product-input-bundles", directory]

    sdk = get_fuchsia_sdk_toolchain(ctx)
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = ctx.files.packages + ctx.files.product_configuration + ctx.files.product_input_bundles,
        outputs = [product_config_dir],
        mnemonic = "HybridProductConfig",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset([product_config_dir])),
        FuchsiaProductConfigInfo(
            directory = product_config_dir.path,
            build_type = ctx.attr.product_configuration[FuchsiaProductConfigInfo].build_type,
            build_id_dirs = [],
        ),
    ]

fuchsia_hybrid_product_configuration = rule(
    doc = "Combine in-tree packages with a prebuilt product config from out of tree for hybrid assembly",
    implementation = _fuchsia_hybrid_product_configuration_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaProductConfigInfo],
    attrs = {
        "product_configuration": attr.label(
            doc = "Prebuilt product config",
            providers = [FuchsiaProductConfigInfo],
            mandatory = True,
        ),
        "packages": attr.label_list(
            doc = "List of packages to replace. The packages are replaced by their name.",
            providers = [FuchsiaPackageInfo],
            default = [],
        ),
        "product_input_bundles": attr.label_list(
            doc = "List of product input bundles to replace. The PIBs are replaced by their name.",
            providers = [FuchsiaProductInputBundleInfo],
            default = [],
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def _fuchsia_prebuilt_product_configuration_extract_package_impl(ctx):
    _package_manifest = ctx.actions.declare_file(ctx.label.name + "_out/package_manifest.json")
    _meta_far = ctx.actions.declare_file("meta.far", sibling = _package_manifest)
    _package_dir = ctx.actions.declare_directory(ctx.label.name, sibling = _package_manifest)

    _inputs = ctx.files.product_configuration

    _outputs = [
        _package_manifest,
        _package_dir,
        _meta_far,
    ]

    product_config = ctx.attr.product_configuration[FuchsiaProductConfigInfo]
    args = [
        "extract",
        "product-package",
        "--config",
        product_config.directory,
        "--package-name",
        ctx.attr.package_name,
        "--outdir",
        _package_dir.path,
        "--output-package-manifest",
        _package_manifest.path,
    ]

    sdk = get_fuchsia_sdk_toolchain(ctx)
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = _inputs,
        outputs = _outputs,
        mnemonic = "ProductConfigExtractPackage",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [
        DefaultInfo(files = depset(direct = _outputs + _inputs)),
        FuchsiaPackageInfo(
            package_manifest = _package_manifest,
            meta_far = _meta_far,
            files = _inputs + _outputs,
            package_resources = [],
        ),
    ]

fuchsia_prebuilt_product_configuration_extract_package = rule(
    doc = "Extract a package from a prebuilt product config",
    implementation = _fuchsia_prebuilt_product_configuration_extract_package_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaPackageInfo],
    attrs = {
        "product_configuration": attr.label(
            doc = "Prebuilt product config",
            providers = [FuchsiaProductConfigInfo],
            mandatory = True,
        ),
        "package_name": attr.string(
            doc = "Name of the package to extract",
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
