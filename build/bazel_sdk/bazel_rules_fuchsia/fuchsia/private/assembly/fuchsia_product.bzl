# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for assembling a Fuchsia product."""

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("@fuchsia_rules_common//assembly:providers.bzl", "FuchsiaProductConfigInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:ffx_tool.bzl", "get_ffx_assembly_args", "get_ffx_assembly_inputs")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    ":providers.bzl",
    "FuchsiaAssemblyDeveloperOverridesInfo",
    "FuchsiaAssemblyDeveloperOverridesListInfo",
    "FuchsiaBoardConfigInfo",
    "FuchsiaPlatformArtifactsInfo",
    "FuchsiaProductAssemblyInfo",
    "FuchsiaProductImageInfo",
)

def _match_assembly_pattern_string(label, pattern):
    package = label.package
    assembly_pattern = pattern.removeprefix("//")
    if assembly_pattern.endswith("/*"):
        # Match any target in the package or below
        return (package + "/").startswith(assembly_pattern.removesuffix("*"))
    elif assembly_pattern.endswith(":*"):
        # Match package exactly.
        return package == assembly_pattern.removesuffix(":*")
    else:
        # Match label exactly.
        return assembly_pattern == "%s:%s" % (package, label.name)

def _fuchsia_product_assembly_impl(ctx):
    fuchsia_toolchain = get_fuchsia_sdk_toolchain(ctx)
    platform_artifacts = ctx.attr.platform_artifacts[FuchsiaPlatformArtifactsInfo]
    out_dir = ctx.actions.declare_directory(ctx.label.name + "_out")
    platform_aibs_file = ctx.actions.declare_file(ctx.label.name + "_platform_assembly_input_bundles.json")

    # Create platform_assembly_input_bundles.json file
    ctx.actions.run(
        outputs = [platform_aibs_file],
        inputs = platform_artifacts.files,
        executable = ctx.executable._create_platform_aibs_file,
        arguments = [
            "--platform-aibs",
            platform_artifacts.root,
            "--output",
            platform_aibs_file.path,
        ],
        mnemonic = "Assembly",
        progress_message = "Gathering AIBs for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    # Invoke Product Assembly
    product_config = ctx.attr.product_config[FuchsiaProductConfigInfo]
    board_config = ctx.attr.board_config[FuchsiaBoardConfigInfo]

    build_type = product_config.build_type
    build_id_dirs = []
    build_id_dirs += product_config.build_id_dirs
    build_id_dirs += board_config.build_id_dirs

    inputs_also_needed_by_create_system = []
    inputs_also_needed_by_create_system += ctx.files.product_config
    inputs_also_needed_by_create_system += ctx.files.board_config
    inputs_also_needed_by_create_system += platform_artifacts.files

    ffx_inputs = get_ffx_assembly_inputs(fuchsia_toolchain)
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_ffx_isolate_dir")

    ffx_invocation = get_ffx_assembly_args(fuchsia_toolchain)

    # Enable the use of the example AIBs when the product configuration
    # specifies that it needs it (this isn't available to SDK rules)
    if getattr(product_config, "enable_example_aib", False):
        ffx_invocation += [
            "--config",
            "assembly_example_enabled=true",
        ]

    ffx_invocation += [
        "--isolate-dir",
        ffx_isolate_dir.path,
        "assembly",
        "product",
        "--product",
        product_config.directory,
        "--board-config",
        board_config.directory,
        "--input-bundles-dir",
        platform_artifacts.root,
        "--outdir",
        out_dir.path,
        "--gendir",
        # Reuse --outdir since it is not actively used in the 'product' subcommmand.
        out_dir.path,
        "--package-validation",
        ctx.attr.package_validation,
    ]

    # Add developer overrides manifest and inputs if necessary.
    overrides_maps = ctx.attr._developer_overrides_list[FuchsiaAssemblyDeveloperOverridesListInfo].maps
    for (pattern_string, overrides_label) in overrides_maps.items():
        if _match_assembly_pattern_string(ctx.label, pattern_string):
            overrides_info = overrides_label[FuchsiaAssemblyDeveloperOverridesInfo]
            inputs_also_needed_by_create_system += overrides_info.inputs
            ffx_invocation.extend([
                "--developer-overrides",
                overrides_info.manifest.path,
            ])

    _ffx_invocation = []
    _ffx_invocation.extend(ffx_invocation)
    shell_src = [
        "set -e",
        "mkdir -p " + ffx_isolate_dir.path,
        " ".join(_ffx_invocation),
    ]

    ctx.actions.run_shell(
        inputs = ffx_inputs + inputs_also_needed_by_create_system,
        outputs = [
            out_dir,
            # Isolate dirs contain useful debug files like logs, so include it
            # in outputs.
            ffx_isolate_dir,
        ],
        command = "\n".join(shell_src),
        mnemonic = "Assembly",
        progress_message = "Product Assembly for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset([out_dir])),
        OutputGroupInfo(
            debug_files = depset([ffx_isolate_dir]),
            all_files = depset([out_dir]),
        ),
        FuchsiaProductAssemblyInfo(
            product_assembly_out = out_dir,
            product_assembly_inputs = depset(inputs_also_needed_by_create_system),
            platform_aibs = platform_aibs_file,
            build_type = build_type,
            build_id_dirs = build_id_dirs,
        ),
    ]

_fuchsia_product_assembly = rule(
    doc = """Declares a target to product a fully-configured list of artifacts that make up a product.""",
    implementation = _fuchsia_product_assembly_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaProductAssemblyInfo],
    attrs = {
        "product_config": attr.label(
            doc = "Product configuration used to assemble this product.",
            providers = [FuchsiaProductConfigInfo],
            mandatory = True,
        ),
        "board_config": attr.label(
            doc = "Board configuration used to assemble this product.",
            providers = [FuchsiaBoardConfigInfo],
            mandatory = True,
        ),
        "platform_artifacts": attr.label(
            doc = "Platform artifacts to use for this product.",
            providers = [FuchsiaPlatformArtifactsInfo],
            mandatory = True,
        ),
        "package_validation": attr.string(
            doc = """Whether package validation errors should be treated as
            errors or warnings. Ignoring validation errors may lead to a buggy
            or nonfunctional product!""",
            default = "error",
            values = ["error", "warning"],
        ),
        "_create_package_manifest_list": attr.label(
            default = "//fuchsia/tools:create_package_manifest_list",
            executable = True,
            cfg = "exec",
        ),
        "_create_platform_aibs_file": attr.label(
            default = "//fuchsia/tools:create_platform_aibs_file",
            executable = True,
            cfg = "exec",
        ),
        "_developer_overrides_list": attr.label(
            default = "//fuchsia:assembly_developer_overrides_list",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def _fuchsia_product_create_system_impl(ctx):
    fuchsia_toolchain = get_fuchsia_sdk_toolchain(ctx)
    platform_artifacts = ctx.attr.platform_artifacts[FuchsiaPlatformArtifactsInfo]
    out_dir = ctx.actions.declare_directory(ctx.label.name + "_out")
    gen_dir_path = "{basedir}/{label_name}_gen".format(
        basedir = out_dir.dirname,
        label_name = ctx.label.name,
    )

    # Data from product assembly
    product_assembly_info = ctx.attr.product_assembly[FuchsiaProductAssemblyInfo]

    # Directory for writing logs and such into
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_ffx_isolate_dir")

    create_system_call_info = generate_create_system_call_info(
        fuchsia_toolchain,
        product_assembly_info,
        ffx_isolate_dir,
        platform_artifacts.root,
        out_dir.path,
        gen_dir_path,
    )

    shell_src = [
        "set -e",
        "mkdir -p " + ffx_isolate_dir.path,
    ] + create_system_call_info.shell_src

    ctx.actions.run_shell(
        inputs = create_system_call_info.inputs,
        outputs = [
            out_dir,
            # Isolate dirs contain useful debug files like logs, so include it
            # in outputs.
            ffx_isolate_dir,
        ],
        command = "\n".join(shell_src),
        mnemonic = "Assembly",
        progress_message = "Assembly Create-system for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )
    outputs = [out_dir]
    return [
        DefaultInfo(files = depset(outputs)),
        OutputGroupInfo(
            debug_files = depset([ffx_isolate_dir]),
            all_files = depset(outputs),
        ),
        FuchsiaProductImageInfo(
            images_out = out_dir,
            platform_aibs = ctx.attr.product_assembly[FuchsiaProductAssemblyInfo].platform_aibs,
            product_assembly_out = product_assembly_info.product_assembly_out,
            build_type = product_assembly_info.build_type,
            build_id_dirs = product_assembly_info.build_id_dirs,
        ),
        # Also provide the product assembly info, so that the intermediate rule for the
        # product assembly step doesn't need to be exposed.
        ctx.attr.product_assembly[FuchsiaProductAssemblyInfo],
        ctx.attr.platform_artifacts[FuchsiaPlatformArtifactsInfo],
    ]

def generate_create_system_call_info(
        fuchsia_toolchain,
        product_assembly_info,
        ffx_isolate_dir,
        platform_artifacts,
        out_dir_path,
        gen_dir_path):
    """Returns a struct of the script to run, the inputs, and the outputs for create-system

    This encapsulates the logic for specifying the inputs, outputs, and shell commands to run in
    order to call 'ffx assembly create-system' from a given context.

    This allows us to run the create-system call either as a standalone action, or as part of a
    larger action, while not exposing intermediates to Bazel, as outputs that need to be tracked
    (ie, hashed), saving 10s of seconds on larger (multi-GB) assemblies.

    Args:
      fuchsia_toolchain: The fuchsia toolchain to use
      product_assembly_info: The FuchsiaProductAssemblyInfo for the system being created
      ffx_isolate_dir: directory to write ffx logs into
      platform_artifacts: path to the platform artifacts directory
      out_dir_path: path to the output dir to write the final outputs in
      gen_dir_path: path to the gendir to write temporary files to, these are not available outside
                     of this action

    Returns:
      A struct with the follow named fields:
        inputs: The inputs needed by create-system
        shell_src: The shell command lines to run for the create-system step.
    """
    inputs = [
        product_assembly_info.product_assembly_out,
    ]
    inputs += product_assembly_info.product_assembly_inputs.to_list()
    inputs += get_ffx_assembly_inputs(fuchsia_toolchain)

    ffx_create_system_invocation = get_ffx_assembly_args(fuchsia_toolchain) + [
        "--isolate-dir",
        ffx_isolate_dir.path,
        "assembly",
        "create-system",
        "--image-assembly-config",
        product_assembly_info.product_assembly_out.path + "/image_assembly.json",
        "--platform",
        platform_artifacts,
        "--outdir",
        out_dir_path,
        "--gendir",
        gen_dir_path,
    ]
    shell_src = [" ".join(ffx_create_system_invocation)]
    return struct(
        inputs = inputs,
        shell_src = shell_src,
    )

_fuchsia_product_create_system = rule(
    doc = """Declares a target to generate the images for a Fuchsia product.""",
    implementation = _fuchsia_product_create_system_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaProductImageInfo, FuchsiaProductAssemblyInfo],
    attrs = {
        "product_assembly": attr.label(
            doc = "A fuchsia_product_assembly target.",
            providers = [FuchsiaProductAssemblyInfo],
            mandatory = True,
        ),
        "platform_artifacts": attr.label(
            doc = "Platform artifacts to use for this product.",
            providers = [FuchsiaPlatformArtifactsInfo],
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def fuchsia_product(
        name,
        board_config,
        product_config,
        platform_artifacts = None,
        package_validation = None,
        **kwargs):
    _fuchsia_product_assembly(
        name = name + "_product_assembly",
        board_config = board_config,
        product_config = product_config,
        platform_artifacts = platform_artifacts,
        package_validation = package_validation,
    )

    _fuchsia_product_create_system(
        name = name,
        product_assembly = ":" + name + "_product_assembly",
        platform_artifacts = platform_artifacts,
        **kwargs
    )
