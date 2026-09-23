# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for defining assembly board configuration."""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load(
    "@fuchsia_rules_common//:local_actions.bzl",
    "LOCAL_ONLY_ACTION_KWARGS",
)
load(
    "@fuchsia_rules_common//assembly:json_utils.bzl",
    "extract_labels",
    "replace_labels_with_files",
)
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    "//fuchsia/private/licenses:common.bzl",
    "check_type",
)
load(
    ":providers.bzl",
    "FuchsiaBoardConfigInfo",
    "FuchsiaBoardInputBundleInfo",
    "FuchsiaBoardInputBundleSetInfo",
    "FuchsiaPartitionsConfigInfo",
    "FuchsiaPostProcessingScriptInfo",
)
load(
    ":utils.bzl",
    "select_root_dir_with_file",
)

def _fuchsia_board_configuration_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    build_id_dirs = []

    board_config = {}
    board_config_file = ctx.actions.declare_file(ctx.label.name + "_board_config.json")

    board_config["name"] = ctx.attr.board_name
    board_config["provided_features"] = ctx.attr.provided_features
    board_config["arch"] = ctx.attr.arch

    kernel = json.decode(ctx.attr.kernel)
    check_type(kernel, "dict")
    if kernel != {}:
        board_config["kernel"] = kernel

    platform = json.decode(ctx.attr.platform)
    check_type(platform, "dict")
    if platform != {}:
        board_config["platform"] = platform

    hardware_info = json.decode(ctx.attr.hardware_info)
    check_type(hardware_info, "dict")
    if hardware_info != {}:
        board_config["hardware_info"] = hardware_info

    if ctx.attr.tee_trusted_app_guids:
        board_config["tee_trusted_app_guids"] = ctx.attr.tee_trusted_app_guids

    input_files = []
    input_bundles = {}
    input_files.extend(ctx.files.board_input_bundles)
    for (index, bib) in enumerate(ctx.attr.board_input_bundles):
        input_bundles[str(index)] = bib[FuchsiaBoardInputBundleInfo].directory
        build_id_dirs.extend(bib[FuchsiaBoardInputBundleInfo].build_id_dirs)
    board_config["input_bundles"] = input_bundles

    creation_args = []
    input_files.extend(ctx.files.board_input_bundle_sets)
    for bib_set in ctx.attr.board_input_bundle_sets:
        creation_args += [
            "--board-input-bundle-sets",
            bib_set[FuchsiaBoardInputBundleSetInfo].directory,
        ]
        build_id_dirs.extend(bib_set[FuchsiaBoardInputBundleSetInfo].build_id_dirs)

    if ctx.attr.devicetree:
        input_files.append(ctx.file.devicetree)
        board_config["devicetree"] = ctx.file.devicetree.path

    if ctx.attr.devicetree_overlay:
        input_files.append(ctx.file.devicetree_overlay)
        board_config["devicetree_overlay"] = ctx.file.devicetree_overlay.path

    if ctx.attr.zbi_extra_items:
        zbi_extra_items_file = ctx.actions.declare_file(ctx.label.name + "_zbi_extra_items.zbi")
        zbi_args = ["-o", zbi_extra_items_file.path]
        for f in ctx.files.zbi_extra_items:
            zbi_args += ["--type=container", f.path]

        ctx.actions.run(
            executable = sdk.zbi,
            arguments = zbi_args,
            inputs = ctx.files.zbi_extra_items,
            outputs = [zbi_extra_items_file],
            mnemonic = "FuchsiaBoardZbiExtraItems",
            progress_message = "Combining ZBI extra items for %s" % ctx.label,
        )

        input_files.append(zbi_extra_items_file)
        board_config["zbi_extra_items"] = zbi_extra_items_file.path

    filesystems = json.decode(ctx.attr.filesystems)
    check_type(filesystems, "dict")
    replace_labels_with_files(filesystems, ctx.attr.filesystems_labels)
    board_config["filesystems"] = filesystems

    # Ensure that exactly one of "version" and "version_file" is set.
    # Note: an empty string "" is acceptable: non-official builds may not
    # have access to a version number.
    if ((ctx.attr.version != "__unset") == (ctx.attr.version_file != None)):
        fail("Exactly one of \"version\" or \"version_file\" must be set.")

    # If version is "__unset", the target hasn't set it.
    if ctx.attr.version != "__unset":
        creation_args += ["--version", ctx.attr.version]
    if ctx.file.version_file:
        creation_args += ["--version-file", ctx.file.version_file.path]
        input_files.append(ctx.file.version_file)

    if ctx.attr.repo:
        repo = ctx.attr.repo
    else:
        repo = ctx.attr._release_repository_flag[BuildSettingInfo].value

    # TODO(https://b.corp.google.com/issues/416239346): Make "repo" field
    # required.
    if repo:
        creation_args += ["--repo", repo]

    release_info = {}
    release_info["info"] = {
        "name": ctx.attr.release_info_name if ctx.attr.release_info_name != "__unset" else ctx.attr.board_name,
        "repository": "intermediate_repository",
        "version": "intermediate_version",
    }
    release_info["bib_sets"] = []
    board_config["release_info"] = release_info

    if ctx.attr.post_processing_script:
        script = ctx.attr.post_processing_script[FuchsiaPostProcessingScriptInfo]

        board_script_path = None
        paths_map = {}
        for source, dest in script.post_processing_script_inputs.items():
            # The sources come from two resource: passed in Label or python related files. If
            # the source is a file, we directly use it, otherwise it is a label, we pick
            # the first item of the files, and copy it into board configuration directory.
            if type(source) == "File":
                source_file = source
            else:
                source_file = source.files.to_list()[0]

            script_input = ctx.actions.declare_file(dest)
            ctx.actions.symlink(output = script_input, target_file = source_file)
            input_files.extend([source_file, script_input])

            if dest == script.post_processing_script_path:
                board_script_path = script_input.path
            else:
                paths_map[dest] = script_input.path

        if not board_script_path:
            fail("board_script_path must be present in the inputs.")

        filesystems = board_config.get("filesystems", {})
        board_config["filesystems"] = filesystems

        zbi = filesystems.get("zbi", {})
        zbi["postprocessing_script"] = {
            "board_script_path": board_script_path,
            "args": script.post_processing_script_args,
            "inputs": paths_map,
        }
        board_config["filesystems"]["zbi"] = zbi

    if ctx.attr.partitions_configuration:
        creation_args += [
            "--partitions-config",
            ctx.attr.partitions_configuration[FuchsiaPartitionsConfigInfo].directory,
        ]

    content = json.encode_indent(board_config, indent = "  ")
    ctx.actions.write(board_config_file, content)
    input_files.append(board_config_file)

    board_config_dir = ctx.actions.declare_directory(ctx.label.name)
    args = [
        "generate",
        "board",
        "--config",
        board_config_file.path,
        "--output",
        board_config_dir.path,
    ] + creation_args
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = input_files + ctx.files.filesystems_labels + ctx.files.partitions_configuration,
        outputs = [board_config_dir],
        mnemonic = "FuchsiaBoardConfiguration",
        progress_message = "Creating board config for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(
            files = depset([board_config_dir]),
        ),
        FuchsiaBoardConfigInfo(
            directory = board_config_dir.path,
            build_id_dirs = build_id_dirs,
        ),
        OutputGroupInfo(
            build_id_dirs = depset(transitive = build_id_dirs),
        ),
    ]

_fuchsia_board_configuration = rule(
    doc = """Generates a board configuration file.""",
    implementation = _fuchsia_board_configuration_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "board_name": attr.string(
            doc = "Name of this board.",
            mandatory = True,
        ),
        "arch": attr.string(
            doc = "Architecture of this board.",
            values = ["x64", "arm64", "riscv64"],
            mandatory = True,
        ),
        "hardware_info": attr.string(
            doc = "Data provided via the 'fuchsia.hwinfo.Board' protocol.",
            default = "{}",
        ),
        "board_input_bundles": attr.label_list(
            doc = "Board Input Bundles targets to be included into the board.",
            providers = [FuchsiaBoardInputBundleInfo],
        ),
        "board_input_bundle_sets": attr.label_list(
            doc = "Board Input Bundle Sets to make available in the board.",
            providers = [FuchsiaBoardInputBundleSetInfo],
        ),
        "provided_features": attr.string_list(
            doc = "The features that this board provides to the product.",
        ),
        "filesystems": attr.string(
            doc = "The filesystem configuration options provided by the board.",
            default = "{}",
        ),
        "filesystems_labels": attr.label_keyed_string_dict(
            doc = """Map of labels to LABEL(label) strings in the filesystems config.""",
            allow_files = True,
            default = {},
        ),
        "devicetree": attr.label(
            doc = "Devicetree binary (.dtb) file",
            allow_single_file = True,
        ),
        "devicetree_overlay": attr.label(
            doc = "Devicetree binary overlay (.dtbo) file",
            allow_single_file = True,
        ),
        "zbi_extra_items": attr.label_list(
            doc = "ZBI extra items containers",
            allow_files = True,
        ),
        "post_processing_script": attr.label(
            doc = "The post processing script to be included into the board configuration",
            providers = [FuchsiaPostProcessingScriptInfo],
        ),
        "platform": attr.string(
            doc = "The platform related configuration provided by the board.",
            default = "{}",
        ),
        "kernel": attr.string(
            doc = "The kernel related configuration provided by the board.",
            default = "{}",
        ),
        "tee_trusted_app_guids": attr.string_list(
            doc = "GUIDs for the TAs provided by this board's TEE driver.",
            default = [],
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
        "partitions_configuration": attr.label(
            doc = "The partitions config to include into the board configuration",
            providers = [FuchsiaPartitionsConfigInfo],
        ),
        "_release_repository_flag": attr.label(
            doc = "String flag used to set the name of the release repository.",
            default = "@rules_fuchsia//fuchsia/flags:fuchsia_release_repository",
        ),
        "release_info_name": attr.string(
            doc = "Name of this board. If set, this is used in the 'name' field of the release_info section of the output config.",
            default = "__unset",
        ),
    },
)

def fuchsia_board_configuration(
        name,
        filesystems = {},
        platform = {},
        kernel = {},
        hardware_info = {},
        **kwargs):
    """A board configuration that takes a dict for the filesystems config."""
    filesystem_labels = extract_labels(filesystems)

    _fuchsia_board_configuration(
        name = name,
        filesystems = json.encode_indent(filesystems, indent = "    "),
        platform = json.encode_indent(platform, indent = "    "),
        kernel = json.encode_indent(kernel, indent = "    "),
        hardware_info = json.encode_indent(hardware_info, indent = "    "),
        filesystems_labels = filesystem_labels,
        arch = select({
            "@rules_fuchsia//fuchsia/constraints:cpu_x64": "x64",
            "@rules_fuchsia//fuchsia/constraints:cpu_arm64": "arm64",
            "@rules_fuchsia//fuchsia/constraints:cpu_riscv64": "riscv64",
        }),
        **kwargs
    )

def _fuchsia_prebuilt_board_configuration_impl(ctx):
    directory = select_root_dir_with_file(ctx.files.files, "board_configuration.json")
    return [
        DefaultInfo(files = depset(ctx.files.files)),
        FuchsiaBoardConfigInfo(
            directory = directory,
            build_id_dirs = [],
        ),
    ]

_fuchsia_prebuilt_board_configuration = rule(
    doc = """A prebuilt board configuration file and its main hardware support bundle.""",
    implementation = _fuchsia_prebuilt_board_configuration_impl,
    provides = [FuchsiaBoardConfigInfo],
    attrs = {
        "files": attr.label(
            doc = "All files referenced by the board config.",
            mandatory = True,
        ),
    },
)

def fuchsia_prebuilt_board_configuration(
        *,
        directory = None,
        **kwargs):
    """A board configuration that has been prebuilt and exists in a specific folder."""

    # TODO(chandarren): Migrate users to use `files` instead.
    if directory:
        kwargs["files"] = directory
    _fuchsia_prebuilt_board_configuration(**kwargs)

def _fuchsia_hybrid_board_configuration_impl(ctx):
    board_config_dir = ctx.actions.declare_directory(ctx.label.name)

    creation_inputs = []
    creation_args = []
    build_id_dirs = []

    board_config = ctx.attr.board_configuration[FuchsiaBoardConfigInfo]
    build_id_dirs.extend(board_config.build_id_dirs)

    creation_inputs.extend(ctx.files.replacement_board_input_bundles)
    if ctx.attr.replacement_board_input_bundles:
        board = ctx.attr.replacement_board_input_bundles[FuchsiaBoardConfigInfo]
        creation_args += [
            "--replace-bibs-from-board",
            board.directory,
        ]
        build_id_dirs.extend(board.build_id_dirs)

    creation_inputs.extend(ctx.files.replacement_board_input_bundle_sets)
    for bib_set in ctx.attr.replacement_board_input_bundle_sets:
        bib_set = bib_set[FuchsiaBoardInputBundleSetInfo]
        creation_args += [
            "--replace-bib-sets",
            bib_set.directory,
        ]
        build_id_dirs.extend(bib_set.build_id_dirs)

    if ctx.attr.replacement_devicetree:
        creation_inputs.append(ctx.file.replacement_devicetree)
        creation_args += [
            "--replace-devicetree",
            ctx.file.replacement_devicetree.path,
        ]

    args = [
        "generate",
        "hybrid-board",
        "--config",
        board_config.directory,
        "--output",
        board_config_dir.path,
    ] + creation_args

    sdk = get_fuchsia_sdk_toolchain(ctx)
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = creation_inputs + ctx.files.board_configuration,
        outputs = [board_config_dir],
        mnemonic = "FuchsiaHybridBoardConfiguration",
    )

    return [
        DefaultInfo(files = depset([board_config_dir])),
        FuchsiaBoardConfigInfo(
            directory = board_config_dir.path,
            build_id_dirs = build_id_dirs,
        ),
    ]

fuchsia_hybrid_board_configuration = rule(
    doc = "Combine in-tree board input bundles with a board from out-of-tree for hybrid assembly",
    implementation = _fuchsia_hybrid_board_configuration_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    provides = [FuchsiaBoardConfigInfo],
    attrs = {
        "board_configuration": attr.label(
            doc = "Prebuilt board config",
            providers = [FuchsiaBoardConfigInfo],
            mandatory = True,
        ),
        "replacement_board_input_bundles": attr.label(
            doc = "In-tree board containing input bundles to replace those in `board_configuration`",
            providers = [FuchsiaBoardConfigInfo],
        ),
        "replacement_board_input_bundle_sets": attr.label_list(
            doc = "Board input bundle sets to replace inside the board",
            providers = [FuchsiaBoardInputBundleSetInfo],
        ),
        "replacement_devicetree": attr.label(
            doc = "Devicetree binary (.dtb) file",
            allow_single_file = True,
        ),
    },
)
