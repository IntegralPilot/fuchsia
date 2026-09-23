# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A Bazel rule and symbolic macro for preprocessing and defining version_history.json for SDK.

In practice, there will probably only be one instance of this rule, at
//sdk:version_history, but we'll give it its own macro for tidiness.
"""

load("//build/bazel/rules/idk/private:idk_atom.bzl", "idk_atom")
load(
    "//build/bazel/rules/idk/private:idk_common.bzl",
    "json_encode_dict_values",
)

def _sdk_version_history_gen_impl(ctx):
    output_file = ctx.actions.declare_file(ctx.label.name + ".json")

    args = ctx.actions.args()
    args.add("--input", ctx.file.source)
    args.add("--daily-commit-hash-file", ctx.file._daily_commit_hash_file)
    args.add("--daily-commit-timestamp-file", ctx.file._daily_commit_stamp_file)
    args.add("--output", output_file)

    ctx.actions.run(
        inputs = [
            ctx.file.source,
            ctx.file._daily_commit_hash_file,
            ctx.file._daily_commit_stamp_file,
        ],
        outputs = [output_file],
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "SdkVersionHistory",
        progress_message = "Preprocessing SDK version history for {}".format(ctx.label),
    )

    return [
        DefaultInfo(
            files = depset([output_file]),
        ),
    ]

_sdk_version_history_gen = rule(
    implementation = _sdk_version_history_gen_impl,
    attrs = {
        "source": attr.label(
            doc = "The version_history.json file to preprocess.",
            mandatory = True,
            allow_single_file = True,
        ),
        "_daily_commit_hash_file": attr.label(
            doc = "The daily commit hash file.",
            default = "//build/info:jiri_generated/integration_daily_commit_hash.txt",
            allow_single_file = True,
        ),
        "_daily_commit_stamp_file": attr.label(
            doc = "The daily commit stamp/timestamp file.",
            default = "//build/info:jiri_generated/integration_daily_commit_stamp.txt",
            allow_single_file = True,
        ),
        "_tool": attr.label(
            default = "//build/sdk/generate_version_history:generate_version_history_bin",
            cfg = "exec",
            executable = True,
        ),
    },
)

def _sdk_version_history_impl(name, source, category, id, visibility):
    # The compile target matches the macro name so other rules can consume it directly.
    _sdk_version_history_gen(
        name = name,
        source = source,
        visibility = visibility,
    )

    files_map = {
        "version_history.json": ":" + name,
    }

    additional_prebuild_info_values = {
        "source": "sdk/version_history.json",
        "daily_commit_hash_file": "build/info/jiri_generated/integration_daily_commit_hash.txt",
        "daily_commit_timestamp_file": "build/info/jiri_generated/integration_daily_commit_stamp.txt",
    }

    idk_atom(
        name = name + "_idk",
        idk_name = name,
        id = id,
        meta_dest = "version_history.json",
        type = "version_history",
        category = category,
        stable = True,
        api_area = "Unknown",
        files_map = files_map,
        additional_prebuild_info = json_encode_dict_values(additional_prebuild_info_values),
        target_compatible_with = ["@platforms//os:fuchsia"],
        visibility = visibility,
    )

sdk_version_history = macro(
    doc = """Defines a preprocessing target and an SDK/IDK atom for version_history.json.

    Args:
        name: Target name for the preprocessed output.
        source: Label of the un-preprocessed version_history.json.
        category: Publication category of the atom in the IDK.
        id: Canonical identifier of the element in the IDK.
    """,
    implementation = _sdk_version_history_impl,
    attrs = {
        "source": attr.label(
            doc = "The version_history.json file to preprocess.",
            mandatory = True,
            allow_single_file = True,
        ),
        "category": attr.string(
            doc = "Publication category of the atom in the IDK.",
            mandatory = True,
            values = ["partner"],
            configurable = False,
        ),
        "id": attr.string(
            doc = "Canonical identifier of the element in the IDK.",
            default = "sdk://version_history",
            configurable = False,
        ),
    },
)
