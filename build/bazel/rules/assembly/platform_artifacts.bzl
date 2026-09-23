# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_build_info//:args.bzl", "build_info_version", "target_cpu")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load(
    "@fuchsia_rules_common//assembly:providers.bzl",
    "AssemblyInputBundleInfo",
    "PlatformArtifactsInfo",
)
load("//build/bazel/platforms/transitions:transitions.bzl", "sdk_to_platform_transition")

visibility(["//bundles/assembly", "//build/bazel/assembly/assembly_input_bundles"])

def _platform_artifacts_impl(ctx):
    # The platform_artifacts rule can only be created in the non-sdk
    # `fuchsia_platform` bazel platform configuration.  So validate that here.
    # We don't use the standard 'target_compatible_with = ...' parameter because
    # this allows enforcement on every instance fo the rule, and lets us provide
    # a custom error message that says which targets should be used instead.

    is_fuchsia = ctx.target_platform_has_constraint(
        ctx.attr._fuchsia_constraint[platform_common.ConstraintValueInfo],
    )
    is_without_sdk = ctx.target_platform_has_constraint(
        ctx.attr._without_sdk_rules_constraint[platform_common.ConstraintValueInfo],
    )
    if not is_fuchsia or not is_without_sdk:
        fail("platform_artifacts can only be created in the non-SDK Fuchsia platform " +
             "('fuchsia_platform') configuration.  Use the targets in " +
             "'//build/bazel/assembly/assembly_input_bundles' to reference platform " +
             "artifacts from the SDK platform ('fuchsia_sdk').")

    # Now proceed with the actual implementation

    out_dir = ctx.actions.declare_directory(ctx.label.name + "/platform_artifacts")

    aib_list = []

    for aib in ctx.attr.assembly_input_bundles:
        info = aib[AssemblyInputBundleInfo]
        aib_list.append({
            "name": info.name,
            "path": info.directory,
        })

    aib_list_file = ctx.actions.declare_file(ctx.label.name + "_aib_list.json")
    ctx.actions.write(
        output = aib_list_file,
        content = json.encode_indent(aib_list),
    )

    inputs = [aib_list_file] + ctx.files.assembly_input_bundles

    args = ctx.actions.args()
    args.add("generate")
    args.add("platform-artifacts")
    args.add("--name", target_cpu)
    args.add("--aib-list", aib_list_file.path)
    args.add("--repo", "fuchsia")
    args.add("--version", build_info_version)
    args.add("--output", out_dir.path)

    # A depfile is required by the tool arguments struct
    depfile = ctx.actions.declare_file(ctx.label.name + ".depfile")
    args.add("--depfile", depfile.path)

    # Gather host tools
    host_tools = [
        ctx.file._assembly_tool,
        ctx.file._blobfs_tool,
        ctx.file._fvm_tool,
        ctx.file._zbi_tool,
        ctx.file._cmc_tool,
        ctx.file._fxfs_pbtool,
    ]

    for tool in host_tools:
        inputs.append(tool)
        args.add("--tools", tool)

    ctx.actions.run(
        inputs = inputs,
        outputs = [out_dir, depfile],
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "PlatformArtifacts",
        progress_message = "Generating Platform Artifacts %s" % ctx.label.name,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset([out_dir])),
        PlatformArtifactsInfo(
            root = out_dir.path,
            files = [out_dir],
        ),
    ]

platform_artifacts = rule(
    doc = "Generates a directory of platform artifacts for out-of-tree consumption.",
    implementation = _platform_artifacts_impl,
    attrs = {
        "assembly_input_bundles": attr.label_list(
            mandatory = True,
            providers = [AssemblyInputBundleInfo],
            doc = "List of AIB targets to include.",
        ),
        "_assembly_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/build/assembly/tools/assembly:assembly",
            allow_single_file = True,
        ),
        "_blobfs_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/src/storage/blobfs/tools:blobfs",
            allow_single_file = True,
        ),
        "_fvm_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/src/storage/bin/fvm:fvm",
            allow_single_file = True,
        ),
        "_zbi_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/zircon/tools/zbi:zbi",
            allow_single_file = True,
        ),
        "_cmc_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/tools/cmc:cmc",
            allow_single_file = True,
        ),
        "_fxfs_pbtool": attr.label(
            default = "@gn_targets//toolchain_host_x64/src/storage/fxfs/fxfs_pbtool:fxfs_pbtool",
            allow_single_file = True,
        ),
        "_tool": attr.label(
            default = "@gn_targets//toolchain_host_x64/build/assembly/tools/assembly_config:assembly_config",
            executable = True,
            cfg = "exec",
        ),
        "_fuchsia_constraint": attr.label(
            default = "@platforms//os:fuchsia",
        ),
        "_without_sdk_rules_constraint": attr.label(
            default = "//build/bazel/platforms:fuchsia_artifacts_build_without_sdk_rules",
        ),
    },
)

def _platform_artifacts_from_fuchsia_platform_impl(ctx):
    # While using the transition targets appears to be idempotent, and doesn't cause problems,
    # we'll make sure people using the 'fuchsia_platform' configuration don't use these targets.
    # and instead depend on the ones that are meant to be used from 'fuchsia_sdk'.
    #
    # We don't use the standard 'target_compatible_with = ...' parameter because
    # it doesn't let us provide a custom error message that says which targets
    # should be used instead.

    is_fuchsia = ctx.target_platform_has_constraint(
        ctx.attr._fuchsia_constraint[platform_common.ConstraintValueInfo],
    )
    is_with_sdk = ctx.target_platform_has_constraint(
        ctx.attr._with_sdk_rules_constraint[platform_common.ConstraintValueInfo],
    )
    if not is_fuchsia or not is_with_sdk:
        fail("You're using the targets meant to transition from the 'fuchsia_sdk' Bazel platfrom " +
             "to the 'fuchsia_platform' with a platform other than 'fuchsia_sdk'")

    actual = ctx.attr.platform_artifacts[0]
    return [
        actual[DefaultInfo],
        actual[PlatformArtifactsInfo],
    ]

platform_artifacts_from_fuchsia_platform = rule(
    doc = "Transition target that returns the platform_artifacts from the non-sdk fuchsia_platform configuration, for use in the sdk-based rules.",
    implementation = _platform_artifacts_from_fuchsia_platform_impl,
    attrs = {
        "platform_artifacts": attr.label(
            mandatory = True,
            providers = [DefaultInfo, PlatformArtifactsInfo],
            cfg = sdk_to_platform_transition,
        ),
        "_fuchsia_constraint": attr.label(
            default = "@platforms//os:fuchsia",
        ),
        "_with_sdk_rules_constraint": attr.label(
            default = "//build/bazel/platforms:fuchsia_artifacts_build_with_sdk_rules",
        ),
    },
)
