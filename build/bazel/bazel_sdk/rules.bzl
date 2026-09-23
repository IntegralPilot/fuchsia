# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules used to manage IDK -> SDK conversions."""

load("@fuchsia_build_config//:defs.bzl", "build_config")
load("@fuchsia_build_info//:args.bzl", "sdk_id", "target_cpu")

def _find_root_directory(files, suffix):
    for f in files:
        if f.path.endswith(suffix):
            return f.path.removesuffix(suffix)
    return None

# genrule() cannot deal with TreeArtifacts inputs or outputs
# so a custom rule is needed to invoke the validate_idk
# py_binary() target.
def _validate_idk(ctx):
    output_file = ctx.actions.declare_file(ctx.label.name)

    if len(ctx.files._schema_files) == 0:
        fail("No schema files specified.")
    schema_dir_path = ctx.files._schema_files[0].dirname

    inputs = depset(
        [ctx.file.json_validator, ctx.file.idk_directory_hash] + ctx.files._schema_files,
    )

    args = [
        "--idk-directory",
        ctx.attr.idk_directory,
        "--schema-directory",
        schema_dir_path,
        "--json-validator-path",
        ctx.file.json_validator.path,
        "--stamp-file",
        output_file.path,
    ]

    ctx.actions.run(
        mnemonic = "IDKVerify",
        executable = ctx.executable._validate_idk_script,
        arguments = args,
        inputs = inputs,
        outputs = [output_file],
        execution_requirements = {
            # We want the generated IDK to persist.
            "no-sandbox": "1",

            # A full IDK is currently about 4.7 GiB, and this
            # action runs in a few seconds, so avoid remoting it.
            "no-remote": "1",
            "no-cache": "1",
        },
    )

    return DefaultInfo(files = depset([output_file]))

validate_idk = rule(
    doc = """Generate a merged IDK at build time from `ninja_root_build_dir`.""",
    implementation = _validate_idk,
    attrs = {
        "idk_directory": attr.string(
            doc = "Path to the IDK to validate.",
            mandatory = True,
        ),
        "idk_directory_hash": attr.label(
            doc = "The hash file corresponding to `idk_directory`.",
            mandatory = True,
            allow_single_file = True,
        ),
        "json_validator": attr.label(
            doc = "The JSON validator executable for schema validation",
            default = "//build/tools/json_validator:json_validator_valico",
            allow_single_file = True,
            cfg = "exec",
        ),
        "_schema_files": attr.label(
            doc = "The IDK schema files. They must all be in the same directory.",
            default = "//build/sdk/meta:idk_schema_files",
        ),
        "_validate_idk_script": attr.label(
            default = "//build/sdk/generate_idk:validate_idk",
            executable = True,
            cfg = "exec",
        ),
    },
)

# genrule() cannot deal with TreeArtifacts inputs or outputs
# so a custom rule is needed to invoke the generate_idk_bazel
# py_binary() target.
def _generate_merged_idk(ctx):
    output_dir = ctx.actions.declare_directory(ctx.label.name)

    if len(ctx.files._schema_files) == 0:
        fail("No schema files specified.")
    schema_dir_path = ctx.files._schema_files[0].dirname

    _collection_relative_path = "sdk/exported/%s" % ctx.attr.collection_name

    args = [
        "--collection-relative-path",
        _collection_relative_path,
        "--output-directory",
        output_dir.path,
        "--schema-directory",
        schema_dir_path,
        "--json-validator-path",
        ctx.file.json_validator.path,
        "--host-arch",
        build_config.host_target_triple,
        "--release-version",
        sdk_id,
    ]

    inputs = depset(
        [ctx.file.json_validator] + ctx.files._schema_files,
    )

    for cpu in ctx.attr.buildable_cpus:
        args += [
            "--target-arch",
            cpu,
        ]

    # TODO(https://fxbug.dev/413071161): Either standardize "_for_subbuilds" or
    # allow the prefix to be specified.
    subbuilds_dir_prefix = "idk_subbuild.%s_for_subbuilds" % ctx.attr.collection_name
    root_build_dir = ctx.attr.ninja_root_build_dir
    subbuild_dirs = []

    for cpu in ctx.attr.buildable_cpus:
        # Add subbuilds for the PLATFORM API level.
        if cpu == target_cpu:
            # There is no sub-build for the target CPU - use the collection in
            # the main build directory.
            subbuild_dirs.append(root_build_dir)
        else:
            subbuild_dirs.append("%s/%s-%s" % (root_build_dir, subbuilds_dir_prefix, cpu))

        # Add subbuilds for individual API levels.
        for api_level in ctx.attr.buildable_api_levels:
            subbuild_dirs.append("%s/%s-api%s-%s" % (root_build_dir, subbuilds_dir_prefix, api_level, cpu))

    for dir in subbuild_dirs:
        args += [
            "--subbuild-directory",
            dir,
        ]

    ctx.actions.run(
        mnemonic = "IDKMerge",
        executable = ctx.executable._idk_merge_script,
        arguments = args,
        inputs = inputs,
        outputs = [output_dir],
        execution_requirements = {
            # We want the generated IDK to persist.
            "no-sandbox": "1",

            # A full IDK is currently about 4.7 GiB, and this
            # action runs in a few seconds, so avoid remoting it.
            "no-remote": "1",
            "no-cache": "1",
        },
    )

    return DefaultInfo(files = depset([output_dir]))

generate_merged_idk = rule(
    doc = """Generate a merged IDK at build time from `ninja_root_build_dir`.""",
    implementation = _generate_merged_idk,
    attrs = {
        "collection_name": attr.string(
            doc = "Name of the collection to merge",
            mandatory = True,
        ),
        "buildable_cpus": attr.string_list(
            doc = "List of CPU architectures for which the IDK build will provide build-time support",
            mandatory = True,
        ),
        "buildable_api_levels": attr.string_list(
            doc = "List of API levels for which the IDK build will provide build-time support",
            mandatory = True,
        ),
        "ninja_root_build_dir": attr.string(
            doc = "Path to the Ninja build root directory",
            mandatory = True,
        ),
        "json_validator": attr.label(
            doc = "The JSON validator executable for schema validation",
            default = "//build/tools/json_validator:json_validator_valico",
            allow_single_file = True,
            cfg = "exec",
        ),
        "_schema_files": attr.label(
            doc = "The IDK schema files. They must all be in the same directory.",
            default = "//build/sdk/meta:idk_schema_files",
        ),
        "_idk_merge_script": attr.label(
            default = "//build/sdk/generate_idk:generate_idk_bazel",
            executable = True,
            cfg = "exec",
        ),
    },
)

# genrule() cannot deal with TreeArtifacts inputs or outputs
# so a custom rule is needed to invoke the idk_to_bazel_sdk
# py_binary() target.
def _generate_bazel_sdk(ctx):
    output_dir = ctx.actions.declare_directory(ctx.label.name)

    inputs = [ctx.file._buildifier_tool]

    if ctx.attr.idk_export_dir:
        if ctx.attr.idk_export_label:
            fail("Only define one of idk_export_dir or idk_export_label when calling this rule!")

        input_idk_files = ctx.files.idk_export_dir
        input_idk_path = _find_root_directory(input_idk_files, "/meta/manifest.json")
        if not input_idk_path:
            fail("IDK meta/manifest.json file missing from %s" % ctx.attr.idk_export_dir)
        inputs.extend(input_idk_files)

    elif ctx.attr.idk_export_label:
        # When a build produces a single TreeArtifact, its DefaultInfo.files should be
        # a depset that contains a single File item that covers the whole directory.
        input_idk_depset = ctx.attr.idk_export_label[DefaultInfo].files
        input_idk_files = input_idk_depset.to_list()
        if len(input_idk_files) != 1:
            fail("More than one file listed ad idk_export_label: %s" % ctx.attr.idk.export_label)

        input_idk_path = input_idk_files[0].path
        inputs = depset(inputs, transitive = [input_idk_depset])
    else:
        fail("Define idk_export_dir or idk_export_label when calling this rule!")

    ctx.actions.run(
        mnemonic = "IDKtoBazel",
        executable = ctx.executable._idk_to_bazel_script,
        arguments = [
            "--input-idk",
            input_idk_path,
            "--output-sdk",
            output_dir.path,
            "--buildifier",
            ctx.file._buildifier_tool.path,
        ],
        inputs = inputs,
        outputs = [output_dir],
        execution_requirements = {
            # The input IDK has more than 10,000 files in it, and should
            # always be clean due to the way it is generated by Ninja, so
            # do not use a sandbox to speed up this action.
            "no-sandbox": "1",

            # Similarly, a full IDK is currently about 4.7 GiB, and this
            # action runs in a few seconds, so avoid remoting it.
            "no-remote": "1",
            "no-cache": "1",
        },
    )

    return DefaultInfo(files = depset([output_dir]))

generate_bazel_sdk = rule(
    doc = """Generate a Bazel SDK at build time from an input IDK directory. Setting either idk_export_dir or idk_export_label is mandatory.""",
    implementation = _generate_bazel_sdk,
    attrs = {
        "idk_export_dir": attr.label(
            doc = "Path to IDK export directory, must point to filegroup()",
            allow_files = True,
        ),
        "idk_export_label": attr.label(
            doc = "Label to a target generating an IDK export directory as a TreeArtifact",
        ),
        "_idk_to_bazel_script": attr.label(
            default = "//build/bazel/bazel_sdk:idk_to_bazel_sdk",
            executable = True,
            cfg = "exec",
        ),
        "_buildifier_tool": attr.label(
            default = "//:prebuilt/third_party/buildifier/%s/buildifier" % build_config.host_tag,
            allow_single_file = True,
        ),
    },
)

# Similarly, a genrule() cannot be used here because some attributes can
# point to TreeArtifacts generated by generate_bazel_sdk().
def _compare_bazel_sdk_contents(ctx):
    output_file = ctx.actions.declare_file(ctx.label.name)

    def find_sdk_root_dir(ctx, attr_name):
        attr_value = getattr(ctx.attr, attr_name)
        if attr_value != None and DefaultInfo in attr_value:
            files = attr_value[DefaultInfo].files.to_list()
            if len(files) == 1:
                # This is the case where the label points to a single
                # TreeArtifact generated by generate_bazel_sdk().
                return str(files[0])

            # Otherwise, the label points to a filegroup() that
            # contains lists many IDK files.

        else:
            # This is the case where the label points to a WORKSPACE.bazel
            # file directly.
            files = getattr(ctx.files, attr_name)

        return _find_root_directory(files, "/WORKSPACE.bazel")

    first_sdk_path = find_sdk_root_dir(ctx, "first_sdk")
    if not first_sdk_path:
        fail("Could not find WORKSPACE.bazel from: %s" % ctx.attr.first_sdk)

    second_sdk_path = find_sdk_root_dir(ctx, "second_sdk")
    if not second_sdk_path:
        fail("Could not find WORKSPACE.bazel from: %s" % ctx.attr.second_sdk)

    ctx.actions.run(
        mnemonic = "CompareBazelSDKContents",
        executable = ctx.executable._comparison_script,
        arguments = [
            "--first-sdk",
            first_sdk_path,
            "--second-sdk",
            second_sdk_path,
            "--stamp-file",
            output_file.path,
        ],
        inputs = ctx.files.first_sdk + ctx.files.second_sdk,
        outputs = [output_file],
        execution_requirements = {
            # Disable sandboxing and remoting for the same reason as
            # generate_bazel_sdk().
            "no-sandbox": "1",
            "no-remote": "1",
            "no-cache": "1",
        },
    )
    return DefaultInfo(files = depset([output_file]))

compare_bazel_sdk_contents = rule(
    doc = """Compare the content of two Bazel SDK repositories.""",
    implementation = _compare_bazel_sdk_contents,
    attrs = {
        "first_sdk": attr.label(
            mandatory = True,
            doc = """Label to first SDK target, as generated by generate_bazel_sdk(), or to a corresponding WORKSPACE.bazel file.""",
            allow_files = True,
        ),
        "second_sdk": attr.label(
            doc = """Label to second SDK target, as generated by generate_bazel_sdk(), or to a corresponding WORKSPACE.bazel file.""",
            mandatory = True,
            allow_files = True,
        ),
        "_comparison_script": attr.label(
            default = "//build/bazel/bazel_sdk:compare_bazel_sdks",
            executable = True,
            cfg = "exec",
        ),
    },
)
