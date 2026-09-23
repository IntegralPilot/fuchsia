# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule that creates a product bundle for flashing, emulating, or updating a Fuchsia product to a target device."""

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load(
    "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
    "fuchsia_collect_all_debug_symbols_infos_aspect",
    "transform_collected_debug_symbols_infos",
)
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:ffx_tool.bzl", "get_ffx_product_args", "get_ffx_product_inputs", "get_ffx_scrutiny_args", "get_ffx_scrutiny_inputs")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load("//fuchsia/private/assembly:fuchsia_product.bzl", "generate_create_system_call_info")
load(
    "//fuchsia/private/workflows:fuchsia_product_bundle_tasks.bzl",
    "fuchsia_product_bundle_tasks",
    "product_bundles_help_executable",
)
load(
    ":fuchsia_scrutiny_config.bzl",
    "SCRUTINY_VERIFIERS",
)
load(
    ":providers.bzl",
    "FuchsiaPlatformArtifactsInfo",
    "FuchsiaProductAssemblyInfo",
    "FuchsiaProductBundleInfo",
    "FuchsiaProductImageInfo",
    "FuchsiaRepositoryKeysInfo",
    "FuchsiaScrutinyConfigInfo",
    "FuchsiaSizeCheckerInfo",
    "FuchsiaVirtualDeviceInfo",
)

DELIVERY_BLOB_TYPE = struct(
    UNCOMPRESSED = "0",
    COMPRESSED = "1",
)

def fuchsia_product_bundle(
        *,
        name,
        product_bundle_name = None,
        main = None,
        testonly = None,
        visibility = None,
        tags = [],
        **kwargs):
    """Build a fuchsia product bundle.

    This rule produces a fuchsia product bundle which can be used to flash or OTA a device.

    This macro will expand out into several fuchsia tasks that can be run by a
    bazel invocation. Given a product bundle definition, the following targets will be
    created.

    ```
    fuchsia_product_bundle(
        name = "product_bundle",
        product_bundle_name = "<your_product_name>",
        main = ":your_image",
    )
    ```
    - product_bundle.emu: Starts an emulator with the product_bundle.
    - product_bundle.flash: Flashes a device with the product_bundle.
    - product_bundle.ota: Runs ota on a device with the product_bundle.
    - product_bundle.zip: Creates a zipped version of the product_bundle.
    """

    _build_fuchsia_product_bundle(
        name = name,
        main = main,
        product_bundle_name = product_bundle_name,
        testonly = testonly,
        visibility = visibility,
        tags = tags,
        **kwargs
    )

    _build_zipped_product_bundle(
        name = name + ".zip",
        product_bundle = name,
        testonly = testonly,
        visibility = visibility,
        tags = tags,
    )

    fuchsia_product_bundle_tasks(
        name = "%s_tasks" % name,
        product_bundle = name,
        testonly = testonly,
        visibility = visibility,
        tags = tags,
    )

def _build_zipped_product_bundle_impl(ctx):
    pb_zip = ctx.actions.declare_file(ctx.label.name)
    product_bundle = ctx.attr.product_bundle[FuchsiaProductBundleInfo].product_bundle

    ctx.actions.run_shell(
        inputs = ctx.files.product_bundle,
        outputs = [pb_zip],
        command = """(cd $PRODUCT_BUNDLE_PATH && zip -r pb.zip . 1> /dev/null)
            mv $PRODUCT_BUNDLE_PATH/pb.zip $PB_ZIP_PATH
        """,
        env = {
            "PRODUCT_BUNDLE_PATH": product_bundle.path,
            "PB_ZIP_PATH": pb_zip.path,
        },
        progress_message = "Creating zipped product bundle for %s" % ctx.label.name,
        mnemonic = "CreatePBZip",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [DefaultInfo(files = depset(direct = [pb_zip]))]

_build_zipped_product_bundle = rule(
    doc = """Creates zipped product bundle.""",
    implementation = _build_zipped_product_bundle_impl,
    attrs = {
        "product_bundle": attr.label(
            doc = "The Product Bundle to be zipped",
            providers = [FuchsiaProductBundleInfo],
            mandatory = True,
        ),
    },
)

def _scrutiny_validation(
        ctx,
        fuchsia_toolchain,
        pb_out_dir,
        scrutiny_config,
        platform_scrutiny_config,
        is_recovery = False):
    deps = []
    ffx_scrutiny_inputs = get_ffx_scrutiny_inputs(fuchsia_toolchain)
    ffx_invocation = get_ffx_scrutiny_args(fuchsia_toolchain) + [
        "--isolate-dir $FFX_ISOLATE_DIR",
        "scrutiny",
        "verify",
    ]

    if is_recovery:
        ffx_invocation.append("--recovery")
    label_name = ctx.label.name + ("_recovery" if is_recovery else "")
    if SCRUTINY_VERIFIERS.BOOTFS not in scrutiny_config.excluded_verifiers:
        deps.append(_verify_bootfs_filelist(
            ctx,
            label_name,
            ffx_invocation,
            ffx_scrutiny_inputs,
            pb_out_dir,
            scrutiny_config.bootfs_files + platform_scrutiny_config.bootfs_files,
            scrutiny_config.bootfs_packages + platform_scrutiny_config.bootfs_packages,
        ))
    if SCRUTINY_VERIFIERS.KERNEL_CMDLINE not in scrutiny_config.excluded_verifiers:
        deps.append(_verify_kernel_cmdline(
            ctx,
            label_name,
            ffx_invocation,
            ffx_scrutiny_inputs,
            pb_out_dir,
            scrutiny_config.kernel_cmdline + platform_scrutiny_config.kernel_cmdline,
        ))
    if SCRUTINY_VERIFIERS.STATIC_PKGS not in scrutiny_config.excluded_verifiers:
        deps += _verify_base_packages(
            ctx,
            label_name,
            ffx_invocation,
            ffx_scrutiny_inputs,
            pb_out_dir,
            scrutiny_config.static_packages + platform_scrutiny_config.static_packages,
        )
    if SCRUTINY_VERIFIERS.PRE_SIGNING not in scrutiny_config.excluded_verifiers:
        deps.append(_verify_pre_signing(
            ctx,
            label_name,
            ffx_invocation,
            ffx_scrutiny_inputs,
            pb_out_dir,
            platform_scrutiny_config.pre_signing_policy,
            platform_scrutiny_config.pre_signing_goldens_dir,
            platform_scrutiny_config.pre_signing_goldens,
        ))
    if not is_recovery:
        if SCRUTINY_VERIFIERS.ROUTE_SOURCES not in scrutiny_config.excluded_verifiers:
            routes_config_golden = (
                scrutiny_config.routes_config_golden or
                platform_scrutiny_config.routes_config_golden
            )
            if routes_config_golden:
                deps += _verify_route_sources(
                    ctx,
                    ffx_invocation,
                    ffx_scrutiny_inputs,
                    pb_out_dir,
                    routes_config_golden,
                )
        if SCRUTINY_VERIFIERS.COMPONENT_RESOLVERS not in scrutiny_config.excluded_verifiers:
            deps += _verify_component_resolver_allowlist(
                ctx,
                ffx_invocation,
                ffx_scrutiny_inputs,
                pb_out_dir,
                platform_scrutiny_config.component_resolver_allowlist,
            )
        if SCRUTINY_VERIFIERS.ROUTES not in scrutiny_config.excluded_verifiers:
            deps += _verify_routes(
                ctx,
                ffx_invocation,
                ffx_scrutiny_inputs,
                pb_out_dir,
                platform_scrutiny_config.component_route_exceptions,
                platform_scrutiny_config.component_tree_configs + scrutiny_config.component_tree_configs,
            )
        if SCRUTINY_VERIFIERS.STRUCTURED_CONFIG not in scrutiny_config.excluded_verifiers:
            deps += _verify_structured_config(
                ctx,
                ffx_invocation,
                ffx_scrutiny_inputs,
                pb_out_dir,
                platform_scrutiny_config.structured_config_policy,
                platform_scrutiny_config.component_tree_configs + scrutiny_config.component_tree_configs,
            )

        deps += _extract_structured_config(
            ctx,
            get_ffx_scrutiny_args(fuchsia_toolchain) + ["--isolate-dir $FFX_ISOLATE_DIR"],
            ffx_scrutiny_inputs,
            pb_out_dir,
            is_recovery,
            platform_scrutiny_config.component_tree_configs + scrutiny_config.component_tree_configs,
        )
    return deps

def _verify_bootfs_filelist(
        ctx,
        label_name,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        zbi_bootfs_filelist_goldens,
        zbi_bootfs_packages_goldens):
    stamp_file = ctx.actions.declare_file(label_name + "_bootfs.stamp")
    ffx_isolate_dir = ctx.actions.declare_directory(label_name + "_bootfs.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "bootfs",
        "--product-bundle",
        pb_out_dir.path,
    ]
    for golden_file in zbi_bootfs_filelist_goldens:
        _ffx_invocation += [
            "--golden",
            golden_file.path,
        ]
    for golden_package in zbi_bootfs_packages_goldens:
        _ffx_invocation += [
            "--golden-packages",
            golden_package.path,
        ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir] + zbi_bootfs_filelist_goldens + zbi_bootfs_packages_goldens + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Bootfs file list for %s" % label_name,
        mnemonic = "VerifyBootfsFileList",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return stamp_file

def _verify_kernel_cmdline(
        ctx,
        label_name,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        zbi_kernel_cmdline_goldens):
    stamp_file = ctx.actions.declare_file(label_name + "_kernel.stamp")
    ffx_isolate_dir = ctx.actions.declare_directory(label_name + "_kernel.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "kernel-cmdline",
        "--product-bundle",
        pb_out_dir.path,
    ]
    for golden_file in zbi_kernel_cmdline_goldens:
        _ffx_invocation += [
            "--golden",
            golden_file.path,
        ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir] + zbi_kernel_cmdline_goldens + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Kernel Cmdline for %s" % label_name,
        mnemonic = "VerifyKernelCmdline",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return stamp_file

def _verify_route_sources(
        ctx,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        routes_config_golden):
    stamp_file = ctx.actions.declare_file(ctx.label.name + "_route.stamp")
    tmp_dir = ctx.actions.declare_directory(ctx.label.name + "_route.tmp")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_route.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "--tmp-dir",
        tmp_dir.path,
        "route-sources",
        "--product-bundle",
        pb_out_dir.path,
        "--config",
        routes_config_golden.path,
    ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir, routes_config_golden] + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, tmp_dir, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Route sources for %s" % ctx.label.name,
        mnemonic = "VerifyRouteSource",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [stamp_file, tmp_dir]

def _verify_component_resolver_allowlist(
        ctx,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        component_resolver_allowlist):
    stamp_file = ctx.actions.declare_file(ctx.label.name + "_component_resolver.stamp")
    tmp_dir = ctx.actions.declare_directory(ctx.label.name + "_component_resolver.tmp")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_component_resolver.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "--tmp-dir",
        tmp_dir.path,
        "component-resolvers",
        "--product-bundle",
        pb_out_dir.path,
        "--allowlist",
        component_resolver_allowlist.path,
    ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir, component_resolver_allowlist] + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, tmp_dir, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Component Resolver for %s" % ctx.label.name,
        mnemonic = "VerifyComponentResolver",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [stamp_file, tmp_dir]

def _verify_routes(
        ctx,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        allow_lists,
        component_tree_configs):
    stamp_file = ctx.actions.declare_file(ctx.label.name + "_routes.stamp")
    tmp_dir = ctx.actions.declare_directory(ctx.label.name + "_routes.tmp")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_routes.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "--tmp-dir",
        tmp_dir.path,
        "routes",
        "--product-bundle",
        pb_out_dir.path,
    ]
    for config in component_tree_configs:
        _ffx_invocation += [
            "--component-tree-config",
            config.path,
        ]
    for allow_list in allow_lists:
        _ffx_invocation += [
            "--allowlist",
            allow_list.path,
        ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir] + component_tree_configs + allow_lists + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, tmp_dir, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Routes for %s" % ctx.label.name,
        mnemonic = "VerifyRoutes",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [stamp_file, tmp_dir]

def _verify_base_packages(
        ctx,
        label_name,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        base_packages):
    stamp_file = ctx.actions.declare_file(label_name + "_static_pkgs.stamp")
    tmp_dir = ctx.actions.declare_directory(label_name + "_static_pkgs.tmp")
    ffx_isolate_dir = ctx.actions.declare_directory(label_name + "_static_pkgs.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "--tmp-dir",
        tmp_dir.path,
        "static-pkgs",
        "--product-bundle",
        pb_out_dir.path,
    ]
    for base_package_list in base_packages:
        _ffx_invocation += [
            "--golden",
            base_package_list.path,
        ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir] + base_packages + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, tmp_dir, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify Static pkgs for %s" % label_name,
        mnemonic = "VerifyStaticPkgs",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [stamp_file, tmp_dir]

def _verify_pre_signing(
        ctx,
        label_name,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        pre_signing_policy_file,
        pre_signing_goldens_dir,
        pre_signing_goldens):
    stamp_file = ctx.actions.declare_file(label_name + "_pre_signing.stamp")
    ffx_isolate_dir = ctx.actions.declare_directory(label_name + "_pre_signing.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "pre-signing",
        "--product-bundle",
        pb_out_dir.path,
        "--policy",
        pre_signing_policy_file.path,
        "--golden-files-dir",
        pre_signing_goldens_dir.path,
    ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]

    # The pre_signing_goldens file list is only specified to avoid providing a dir build input
    # to bazel. This avoids the "dependency checking of directories is unsound" warning.
    # The scrutiny invocation itself expects a dir path to be provided.
    inputs = [pb_out_dir, pre_signing_policy_file] + pre_signing_goldens + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify pre-signing checks for %s" % label_name,
        mnemonic = "VerifyPresigningChecks",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return stamp_file

def _verify_structured_config(
        ctx,
        ffx_invocation,
        ffx_scrutiny_inputs,
        pb_out_dir,
        structured_config_policy,
        component_tree_configs):
    stamp_file = ctx.actions.declare_file(ctx.label.name + "_structured_config.stamp")
    tmp_dir = ctx.actions.declare_directory(ctx.label.name + "_structured_config.tmp")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_structured_config.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "--stamp",
        stamp_file.path,
        "--tmp-dir",
        tmp_dir.path,
        "structured-config",
        "--product-bundle",
        pb_out_dir.path,
        "--policy",
        structured_config_policy.path,
    ]
    for config in component_tree_configs:
        _ffx_invocation += [
            "--component-tree-config",
            config.path,
        ]
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir, structured_config_policy] + component_tree_configs + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [stamp_file, tmp_dir, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Verify structured config for %s" % ctx.label.name,
        mnemonic = "VerifyStructedConfig",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [stamp_file, tmp_dir]

def _extract_structured_config(ctx, ffx_invocation, ffx_scrutiny_inputs, pb_out_dir, is_recovery, component_tree_configs):
    structured_config = ctx.actions.declare_file(ctx.label.name + "_structured_config")
    depfile = ctx.actions.declare_file(ctx.label.name + "_depfile")
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_extract_structured_config.ffx")
    _ffx_invocation = ffx_invocation[:] + [
        "scrutiny",
        "extract",
        "structured-config",
        "--product-bundle",
        pb_out_dir.path,
        "--output",
        structured_config.path,
        # These args are currently required, but the outputs are ignored.
        "--build-path",
        pb_out_dir.path,
        "--depfile",
        depfile.path,
    ]
    for config in component_tree_configs:
        _ffx_invocation += [
            "--component-tree-config",
            config.path,
        ]
    if is_recovery:
        _ffx_invocation.append("--recovery")
    script_lines = [
        "set -e",
        " ".join(_ffx_invocation),
    ]
    inputs = [pb_out_dir] + component_tree_configs + ffx_scrutiny_inputs
    ctx.actions.run_shell(
        inputs = inputs,
        outputs = [structured_config, depfile, ffx_isolate_dir],
        env = {"FFX_ISOLATE_DIR": ffx_isolate_dir.path},
        command = "\n".join(script_lines),
        progress_message = "Extract structured config for %s" % ctx.label.name,
        mnemonic = "ExtractStructuredConfig",
        **LOCAL_ONLY_ACTION_KWARGS
    )
    return [structured_config, depfile]

def _build_fuchsia_product_bundle_impl(ctx):
    """Create the product bundle and main image

    This creates the main image, and then uses it with the recovery image to create
    the product bundle, all in a single bazel action, to minimize exposure of the
    (often very large) intermediate directories to Bazel's output hashing.

    Args:
      ctx: the context
    """
    fuchsia_toolchain = get_fuchsia_sdk_toolchain(ctx)

    # For the main product label, the product assembly information is used
    main_product_assembly_info = ctx.attr.main[FuchsiaProductAssemblyInfo]
    main_product_assembly_platform_artifacts = ctx.attr.main[FuchsiaPlatformArtifactsInfo]

    # This is the _only_ outdir that's provided, the intermediates are derived from this:
    pb_outdir = ctx.actions.declare_directory(ctx.label.name + "_out")
    base_outdir = pb_outdir.dirname

    # Build up a single shell script that we'll use to invoke:
    #   - create system (main image)
    #   - product bundle creation

    all_inputs = []
    all_outputs = []

    # Track the build id dirs for both images:
    build_id_dirs = []
    build_id_dirs += main_product_assembly_info.build_id_dirs

    # Isolate dirs contain useful debug files like logs, so include it
    # in outputs.
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_ffx_isolate_dir")
    all_outputs.append(ffx_isolate_dir)

    shell_src = [
        "set -e",
        "mkdir -p " + ffx_isolate_dir.path,
    ]

    # Invoke create system
    create_system_outdir_name = ctx.label.name + "_create_system_out"
    create_system_outdir_path = base_outdir + "/" + create_system_outdir_name
    create_system_gendir_path = base_outdir + "/" + ctx.label.name + "_create_system_gen"
    assembled_system_manifest = ctx.actions.declare_file(create_system_outdir_name + "/assembled_system.json")

    create_system_call_info = generate_create_system_call_info(
        fuchsia_toolchain,
        main_product_assembly_info,
        ffx_isolate_dir,
        main_product_assembly_platform_artifacts.root,
        create_system_outdir_path,
        create_system_gendir_path,
    )

    all_inputs += create_system_call_info.inputs
    shell_src += create_system_call_info.shell_src

    # Invoke product bundle creation
    all_inputs += get_ffx_product_inputs(fuchsia_toolchain)

    pb_outdir = ctx.actions.declare_directory(ctx.label.name + "_out")
    size_report = ctx.actions.declare_file(ctx.label.name + "_size_report.json")
    pb_outputs = [
        pb_outdir,
        size_report,
        assembled_system_manifest,
    ]
    all_outputs += pb_outputs

    product_bundle_name = ctx.attr.product_bundle_name
    delivery_blob_type = ctx.attr.delivery_blob_type

    # This is the base pb creation command, more args will be added based
    # on data from the providers and optional inputs like the recovery image.
    ffx_pb_invocation = get_ffx_product_args(fuchsia_toolchain) + [
        "--isolate-dir",
        ffx_isolate_dir.path,
        "product",
        "create-old",
        "--product-name",
        product_bundle_name,
        "--system-a",
        create_system_outdir_path,
        "--out-dir",
        pb_outdir.path,
        "--gerrit-size-report",
        size_report.path,
    ]

    if delivery_blob_type:
        ffx_pb_invocation.append("--delivery-blob-type " + delivery_blob_type)

    # Add virtual devices.
    for virtual_device in ctx.attr.virtual_devices:
        ffx_pb_invocation.append("--virtual-device")
        ffx_pb_invocation.append(virtual_device[FuchsiaVirtualDeviceInfo].config.path)
        all_inputs.append(virtual_device[FuchsiaVirtualDeviceInfo].config)
    if ctx.attr.default_virtual_device != None:
        ffx_pb_invocation.append("--recommended-device")
        ffx_pb_invocation.append(ctx.attr.default_virtual_device[FuchsiaVirtualDeviceInfo].device_name)

    # If recovery is supplied, add it to the product bundle.
    if ctx.attr.recovery != None:
        system_r_out = ctx.attr.recovery[FuchsiaProductImageInfo].images_out
        build_id_dirs += ctx.attr.recovery[FuchsiaProductImageInfo].build_id_dirs
        ffx_pb_invocation += ["--system-r", system_r_out.path]
        all_inputs.extend(ctx.files.recovery)

    # If update info is supplied, add it to the product bundle.
    if ctx.attr.update_epoch != "":
        if ctx.attr.repository_keys == None:
            fail("Repository keys must be supplied in order to build an update package")
        ffx_pb_invocation += [
            "--update-package-epoch",
            ctx.attr.update_epoch or "1",
        ]

    # If a repository is supplied, add it to the product bundle.
    if ctx.attr.repository_keys != None:
        ffx_pb_invocation += [
            "--tuf-keys",
            ctx.attr.repository_keys[FuchsiaRepositoryKeysInfo].dir,
        ]
        all_inputs += ctx.files.repository_keys

    shell_src.extend([" ".join(ffx_pb_invocation)])

    # Run the whole generated script of actions.
    ctx.actions.run_shell(
        inputs = all_inputs,
        outputs = all_outputs,
        command = "\n".join(shell_src),
        mnemonic = "Assembly",
        progress_message = "Product Image and Bundle Creation for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )
    deps = [pb_outdir, size_report] + create_system_call_info.inputs

    # Scrutiny Validation
    if ctx.attr.main_scrutiny_config:
        build_type = ctx.attr.main[FuchsiaProductImageInfo].build_type
        if build_type == "user":
            platform_scrutiny_config = ctx.attr._platform_user_scrutiny_config[FuchsiaScrutinyConfigInfo]
        elif build_type == "userdebug":
            platform_scrutiny_config = ctx.attr._platform_userdebug_scrutiny_config[FuchsiaScrutinyConfigInfo]
        else:
            fail("scrutiny cannot run on 'product' because it is an eng build type")

        main_scrutiny_config = ctx.attr.main_scrutiny_config[FuchsiaScrutinyConfigInfo]
        deps += _scrutiny_validation(ctx, fuchsia_toolchain, pb_outdir, main_scrutiny_config, platform_scrutiny_config)
    if ctx.attr.recovery_scrutiny_config:
        build_type = ctx.attr.recovery[FuchsiaProductImageInfo].build_type
        if build_type == "user":
            platform_scrutiny_config = ctx.attr._platform_recovery_user_scrutiny_config[FuchsiaScrutinyConfigInfo]
        elif build_type == "userdebug":
            platform_scrutiny_config = ctx.attr._platform_recovery_userdebug_scrutiny_config[FuchsiaScrutinyConfigInfo]
        else:
            fail("scrutiny cannot run on 'recovery' because it is an eng build type")

        recovery_scrutiny_config = ctx.attr.recovery_scrutiny_config[FuchsiaScrutinyConfigInfo]
        deps += _scrutiny_validation(ctx, fuchsia_toolchain, pb_outdir, recovery_scrutiny_config, platform_scrutiny_config, True)

    fuchsia_debug_symbol_info = transform_collected_debug_symbols_infos(
        ctx.attr.main,
        ctx.attr.recovery,
    )

    return [
        DefaultInfo(
            executable = product_bundles_help_executable(ctx),
            files = depset(direct = deps),
        ),
        FuchsiaProductBundleInfo(
            is_remote = False,
            product_bundle = pb_outdir,
            product_bundle_name = product_bundle_name,
            build_id_dirs = build_id_dirs,
        ),
        FuchsiaSizeCheckerInfo(
            size_report = size_report,
        ),
        fuchsia_debug_symbol_info,
        OutputGroupInfo(
            build_id_dirs = depset(transitive = fuchsia_debug_symbol_info.build_id_dirs_mapping.values()),
        ),
    ]

_build_fuchsia_product_bundle = rule(
    doc = """Create a product bundle (PB) for flashing, emulating, or updating a Fuchsia product to a target device.""",
    implementation = _build_fuchsia_product_bundle_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    executable = True,
    attrs = {
        "product_bundle_name": attr.string(
            doc = "Name of the Fuchsia product. E.g. workstation_eng.x64",
        ),
        "delivery_blob_type": attr.string(
            doc = "Delivery blob type of the product bundle.",
            values = [DELIVERY_BLOB_TYPE.UNCOMPRESSED, DELIVERY_BLOB_TYPE.COMPRESSED],
        ),
        "main": attr.label(
            doc = "fuchsia_product target to put in slot A.",
            providers = [FuchsiaProductImageInfo],
            aspects = [fuchsia_collect_all_debug_symbols_infos_aspect],
        ),
        "recovery": attr.label(
            doc = "fuchsia_product target to put in slot R.",
            providers = [FuchsiaProductImageInfo],
            aspects = [fuchsia_collect_all_debug_symbols_infos_aspect],
        ),
        "repository_keys": attr.label(
            doc = "A fuchsia_repository_keys target, which must be specified when update_version_file is specified.",
            providers = [FuchsiaRepositoryKeysInfo],
            default = None,
        ),
        "update_epoch": attr.string(
            doc = "Epoch needed to create update package.",
        ),
        "main_scrutiny_config": attr.label(
            doc = "Scrutiny config for slot A.",
            providers = [FuchsiaScrutinyConfigInfo],
        ),
        "recovery_scrutiny_config": attr.label(
            doc = "Scrutiny config for recovery.",
            providers = [FuchsiaScrutinyConfigInfo],
        ),
        "virtual_devices": attr.label_list(
            doc = "Virtual devices to make available.",
            providers = [FuchsiaVirtualDeviceInfo],
            default = [],
        ),
        "default_virtual_device": attr.label(
            doc = "Default virtual device to run when none is specified.",
            providers = [FuchsiaVirtualDeviceInfo],
            default = None,
        ),
        "_create_package_manifest_list": attr.label(
            default = "//fuchsia/tools:create_package_manifest_list",
            executable = True,
            cfg = "exec",
        ),
        "_platform_user_scrutiny_config": attr.label(
            doc = "Scrutiny config listing platform content of user products",
            providers = [FuchsiaScrutinyConfigInfo],
            default = "//fuchsia/private/assembly/goldens:user",
        ),
        "_platform_userdebug_scrutiny_config": attr.label(
            doc = "Scrutiny config listing platform content of userdebug products",
            providers = [FuchsiaScrutinyConfigInfo],
            default = "//fuchsia/private/assembly/goldens:userdebug",
        ),
        "_platform_recovery_user_scrutiny_config": attr.label(
            doc = "Scrutiny config listing platform content of recovery user products",
            providers = [FuchsiaScrutinyConfigInfo],
            default = "//fuchsia/private/assembly/goldens:user_recovery",
        ),
        "_platform_recovery_userdebug_scrutiny_config": attr.label(
            doc = "Scrutiny config listing platform content of recovery userdebug products",
            providers = [FuchsiaScrutinyConfigInfo],
            default = "//fuchsia/private/assembly/goldens:userdebug_recovery",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
