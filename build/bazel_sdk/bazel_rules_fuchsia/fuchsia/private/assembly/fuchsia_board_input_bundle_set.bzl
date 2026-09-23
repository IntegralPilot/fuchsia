# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for defining assembly board input bundle set."""

load("@bazel_skylib//rules:common_settings.bzl", "BuildSettingInfo")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":providers.bzl", "FuchsiaBoardInputBundleInfo", "FuchsiaBoardInputBundleSetInfo")
load(":utils.bzl", "select_root_dir_with_file")

def _fuchsia_board_input_bundle_set_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    creation_args = []
    creation_inputs = []
    build_id_dirs = []

    creation_inputs.extend(ctx.files.board_input_bundles)
    for bib in ctx.attr.board_input_bundles:
        bib_info = bib[FuchsiaBoardInputBundleInfo]
        creation_args += [
            "--board-input-bundles",
            bib_info.directory,
        ]
        build_id_dirs += bib_info.build_id_dirs

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

    # Create Board Input Bundle Set
    board_input_bundle_set_dir = ctx.actions.declare_directory(ctx.label.name)
    args = [
        "generate",
        "board-input-bundle-set",
        "--name",
        ctx.label.name,
        "--output",
        board_input_bundle_set_dir.path,
    ] + creation_args
    ctx.actions.run(
        executable = sdk.assembly_config,
        arguments = args,
        inputs = creation_inputs,
        outputs = [board_input_bundle_set_dir],
        progress_message = "Creating board input bundle set for %s" % ctx.label.name,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(
            files = depset([board_input_bundle_set_dir]),
        ),
        FuchsiaBoardInputBundleSetInfo(
            directory = board_input_bundle_set_dir.path,
            build_id_dirs = build_id_dirs,
        ),
        OutputGroupInfo(
            build_id_dirs = depset(transitive = build_id_dirs),
        ),
    ]

fuchsia_board_input_bundle_set = rule(
    doc = """Generates a board input bundle set.""",
    implementation = _fuchsia_board_input_bundle_set_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "board_input_bundles": attr.label_list(
            doc = "The board input bundles to include in the set.",
            providers = [FuchsiaBoardInputBundleInfo],
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

def _fuchsia_prebuilt_board_input_bundle_set_impl(ctx):
    directory = select_root_dir_with_file(ctx.files.files, "board_input_bundle_set.json")
    return [
        DefaultInfo(files = depset(ctx.files.files)),
        FuchsiaBoardInputBundleSetInfo(
            directory = directory,
            build_id_dirs = [],
        ),
    ]

fuchsia_prebuilt_board_input_bundle_set = rule(
    doc = """Defines a Board Input Bundle Set based on preexisting files.""",
    implementation = _fuchsia_prebuilt_board_input_bundle_set_impl,
    attrs = {
        "files": attr.label(
            doc = "All files belonging to the Board Input Bundle Set",
            mandatory = True,
        ),
    },
)
