# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common product configuration rules and macros."""

load(
    "@fuchsia_rules_common//:local_actions.bzl",
    "LOCAL_ONLY_ACTION_KWARGS",
)
load(
    "@fuchsia_rules_common//packages:providers.bzl",
    "FuchsiaPackageInfo",
)
load(
    "@fuchsia_rules_common//packages:utils.bzl",
    "get_driver_component_manifests",
)
load(
    ":json_utils.bzl",
    "replace_labels_with_files",
)
load(
    ":providers.bzl",
    "FuchsiaAssembledPackageInfo",
    "FuchsiaOmahaOtaConfigInfo",
    "FuchsiaProductConfigInfo",
    "FuchsiaProductInputBundleInfo",
    "FuchsiaStarnixContainerInfo",
)

# Define build types
BUILD_TYPES = struct(
    ENG = "eng",
    USER = "user",
    USER_DEBUG = "userdebug",
)

# Define the feature set levels available for products
FEATURE_SET_LEVELS = struct(
    EMBEDDABLE = "embeddable",
    BOOTSTRAP = "bootstrap",
    UTILITY = "utility",
    STANDARD = "standard",
)

COMMON_PRODUCT_ASSEMBLY_ATTRIBUTES = {
    "product_config": attr.string(
        doc = "Raw json config. Used as a base template for the config.",
        default = "{}",
    ),
    "product_config_labels": attr.label_keyed_string_dict(
        doc = "Map of labels in the raw json config to LABEL(label) strings. Labels in the raw json config are replaced by file paths identified by their corresponding values in this dict.",
        allow_files = True,
        default = {},
    ),
    "bootfs_packages": attr.label_list(
        doc = "Fuchsia packages to be included in bootfs.",
        providers = [
            [FuchsiaAssembledPackageInfo],
            [FuchsiaPackageInfo],
        ],
        default = [],
    ),
    "base_packages": attr.label_list(
        doc = "Fuchsia packages to be included in base.",
        providers = [
            [FuchsiaAssembledPackageInfo],
            [FuchsiaPackageInfo],
        ],
        default = [],
    ),
    "cache_packages": attr.label_list(
        doc = "Fuchsia packages to be included in cache.",
        providers = [
            [FuchsiaAssembledPackageInfo],
            [FuchsiaPackageInfo],
        ],
        default = [],
    ),
    "base_driver_packages": attr.label_list(
        doc = "Base-driver packages to include in product.",
        providers = [FuchsiaPackageInfo],
        default = [],
    ),
    "product_input_bundles": attr.label_list(
        doc = "Product input bundles to include.",
        providers = [FuchsiaProductInputBundleInfo],
        default = [],
    ),
    "ota_configuration": attr.label(
        doc = "OTA configuration to include in the product. Only for use with products that use Omaha.",
        providers = [FuchsiaOmahaOtaConfigInfo],
    ),
    "version": attr.string(
        doc = "Release version of this board.",
        default = "__unset",
    ),
    "version_file": attr.label(
        doc = "Path to a file containing the current release version.",
        allow_single_file = True,
    ),
    "repo": attr.string(
        doc = "Name of the release repository. Overrides _release_repository_flag when set.",
    ),
    "deps": attr.label_list(
        doc = "Additional dependencies that must be built before this target is built.",
        default = [],
    ),
    "starnix_containers": attr.label_list(
        doc = "Starnix container generation fields needed",
        providers = [FuchsiaStarnixContainerInfo],
        default = [],
    ),
}

def common_product_configuration_impl(ctx, assembly_config_binary, bootfs_files_package = None, repo_name = ""):
    """Common implementation for product configuration rules.

    Args:
        ctx: The context of the rule.
        assembly_config_binary: The path for the assembly_config tool.
        bootfs_files_package: FuchsiaPackageInfo provider instance for a package containing bootfs files.
        repo_name: The name of the repository that the product's packages will be published to.

    Returns:
        A list of providers for the rule.
    """
    product_config = json.decode(ctx.attr.product_config)
    product_config_file = ctx.actions.declare_file(ctx.label.name + "_product_config.json")

    replace_labels_with_files(product_config, ctx.attr.product_config_labels)

    platform = product_config.get("platform", {})
    build_type = platform.get("build_type")
    product = product_config.get("product", {})
    packages = {}

    input_files = []
    build_id_dirs = []
    bootfs_pkg_details = []
    for dep in ctx.attr.bootfs_packages:
        bootfs_pkg_details.append(_create_pkg_detail(dep))
        input_files += _collect_package_file_deps(dep)
        build_id_dirs += _collect_debug_symbols(dep)
    if bootfs_pkg_details:
        packages["bootfs"] = bootfs_pkg_details

    # This is passed as Provider, not a Target, so we directly access its fields.
    if bootfs_files_package:
        bootfs_files_package_info = bootfs_files_package[FuchsiaPackageInfo]
        input_files += bootfs_files_package_info.files
        build_id_dirs += bootfs_files_package_info.build_id_dirs
        product["bootfs_files_package"] = bootfs_files_package_info.package_manifest.path

    base_pkg_details = []
    for dep in ctx.attr.base_packages:
        base_pkg_details.append(_create_pkg_detail(dep))
        input_files += _collect_package_file_deps(dep)
        build_id_dirs += _collect_debug_symbols(dep)
    packages["base"] = base_pkg_details

    cache_pkg_details = []
    for dep in ctx.attr.cache_packages:
        cache_pkg_details.append(_create_pkg_detail(dep))
        input_files += _collect_package_file_deps(dep)
        build_id_dirs += _collect_debug_symbols(dep)
    packages["cache"] = cache_pkg_details
    product["packages"] = packages

    base_driver_details = []
    for dep in ctx.attr.base_driver_packages:
        package_detail = _create_pkg_detail(dep)
        base_driver_details.append(
            {
                "package": package_detail["manifest"],
                "components": get_driver_component_manifests(dep),
            },
        )
        input_files += _collect_package_file_deps(dep)
    product["base_drivers"] = base_driver_details

    starnix_containers = []
    for container in ctx.attr.starnix_containers:
        container_detail = container[FuchsiaStarnixContainerInfo]

        images = {}
        if container_detail.system:
            images["system"] = container_detail.system
        if container_detail.vendor:
            images["vendor"] = container_detail.vendor
        if container_detail.ramdisk:
            images["ramdisk"] = container_detail.ramdisk

        overwrites = zip(
            container_detail.system_file_overwrite_srcs,
            container_detail.system_file_overwrite_dsts,
        )

        creates = zip(
            container_detail.system_file_create_srcs,
            container_detail.system_file_create_dsts,
        )

        container_file_overrides = []

        for (src, dst) in overwrites:
            container_file_overrides.append({
                "image_name": "system",
                "file_path": dst,
                "operation": {
                    "overwrite": src.path,
                },
            })

        for (src, dst) in creates:
            container_file_overrides.append({
                "image_name": "system",
                "file_path": dst,
                "operation": {
                    "create": src.path,
                },
            })

        for delete in container_detail.system_file_override_deletions:
            container_file_overrides.append({
                "image_name": "system",
                "file_path": delete,
                "operation": "remove",
            })

        starnix_containers.append(
            {
                "name": container_detail.name,
                "base": container_detail.base,
                "fstab": container_detail.fstab,
                "init": container_detail.init,
                "hals": container_detail.hals,
                "skip_subpackages": container_detail.skip_subpackages,
                "images_or_package": {"images": images},
                "file_overrides": container_file_overrides,
            },
        )

    if len(starnix_containers) > 0:
        product["starnix_containers"] = starnix_containers

    product_config["product"] = product

    if ctx.attr.ota_configuration:
        swd_config = product_config["platform"].setdefault("software_delivery", {})
        update_checker_config = swd_config.setdefault("update_checker", {})
        omaha_config = update_checker_config.setdefault("omaha_client", {})

        ota_config_info = ctx.attr.ota_configuration[FuchsiaOmahaOtaConfigInfo]

        channels_file = ctx.actions.declare_file("channel_config.json")
        ctx.actions.write(channels_file, ota_config_info.channels)
        input_files.append(channels_file)

        omaha_config["channels_path"] = channels_file.path

        tuf_config_paths = []
        for (hostname, repo_config) in ota_config_info.tuf_repositories.items():
            repo_config_file = ctx.actions.declare_file(hostname + ".json")
            ctx.actions.write(repo_config_file, repo_config)
            tuf_config_paths.append(repo_config_file.path)
            input_files.append(repo_config_file)
        swd_config["tuf_config_paths"] = tuf_config_paths

    content = json.encode_indent(product_config, indent = "  ")
    ctx.actions.write(product_config_file, content)
    input_files.append(product_config_file)

    product_config_dir = ctx.actions.declare_directory(ctx.label.name)
    args = [
        "generate",
        "product",
        "--config",
        product_config_file.path,
        "--output",
        product_config_dir.path,
    ]

    if repo_name:
        args += ["--repo", repo_name]

    if ctx.attr.version != "__unset":
        args += ["--version", ctx.attr.version]
    if ctx.file.version_file:
        args += ["--version-file", ctx.file.version_file.path]
        input_files.append(ctx.file.version_file)

    for pib in ctx.attr.product_input_bundles:
        directory = pib[FuchsiaProductInputBundleInfo].directory
        args += ["--product-input-bundles", directory]

    ctx.actions.run(
        executable = assembly_config_binary,
        arguments = args,
        inputs = input_files + ctx.files.product_config_labels + ctx.files.product_input_bundles + ctx.files.deps + ctx.files.starnix_containers,
        outputs = [product_config_dir],
        progress_message = "Creating product config for %s" % ctx.label.name,
        mnemonic = "FuchsiaProductConfig",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset([product_config_dir])),
        FuchsiaProductConfigInfo(
            directory = product_config_dir.path,
            build_type = build_type,
            build_id_dirs = build_id_dirs,
            enable_example_aib = getattr(ctx.attr, "enable_example_aib", False),
        ),
    ]

def _collect_debug_symbols(dep):
    if FuchsiaPackageInfo in dep:
        return getattr(dep[FuchsiaPackageInfo], "build_id_dirs", [])
    return getattr(dep[FuchsiaAssembledPackageInfo], "build_id_dirs", [])

def _create_pkg_detail(dep):
    """Creates a dictionary with package details from a dependency.

    This function extracts the package manifest path and any associated
    configuration data from a dependency target. It handles dependencies
    with either `FuchsiaPackageInfo` or `FuchsiaAssembledPackageInfo` providers.

    Args:
        dep: A dependency target that has either a `FuchsiaPackageInfo` or
            `FuchsiaAssembledPackageInfo` provider.

    Returns:
        A dictionary containing the package manifest path. If the dependency has
        configuration data, the dictionary will also include a 'config_data'
        key with a list of configuration objects.
    """
    package_manifest_path = None
    configs = None

    # Find the package manifest and configs from the input depending on the provider.
    if FuchsiaPackageInfo in dep:
        package_manifest_path = dep[FuchsiaPackageInfo].package_manifest.path
    elif FuchsiaAssembledPackageInfo in dep:
        package_manifest_path = dep[FuchsiaAssembledPackageInfo].package.package_manifest.path
        configs = dep[FuchsiaAssembledPackageInfo].configs
    else:
        fail("Dependency {} does not have FuchsiaPackageInfo or FuchsiaAssembledPackageInfo provider".format(dep.label))

    # If we have configs, return them.
    if configs:
        config_data = []
        for config in configs:
            config_data.append(
                {
                    "destination": config.destination,
                    "source": config.source.path,
                },
            )
        return {"manifest": package_manifest_path, "config_data": config_data}
    else:
        return {"manifest": package_manifest_path}

def _collect_package_file_deps(dep):
    if FuchsiaPackageInfo in dep:
        return dep[FuchsiaPackageInfo].files

    return dep[FuchsiaAssembledPackageInfo].files
