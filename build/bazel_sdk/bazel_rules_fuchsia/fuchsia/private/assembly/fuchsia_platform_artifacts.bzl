# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for wrapping prebuilt platform artifacts."""

load("@bazel_skylib//lib:paths.bzl", "paths")
load(":providers.bzl", "FuchsiaPlatformArtifactsInfo")
load(":utils.bzl", "select_multiple_files")

def _fuchsia_platform_artifacts_impl(ctx):
    if ctx.file.directory:
        root_dir = ctx.file.directory.path
    else:
        aib_manifests = select_multiple_files(
            ctx.files.files,
            "assembly_config.json",
            "If it's not possible to provide any `assembly_config.json` files directly as inputs, try specifying `directory` manually.",
        )

        # Compute dirname(<platform_artifacts_dir>/aib_dir/assembly_config.json).
        aib_directories = [aib_manifest.dirname for aib_manifest in aib_manifests]

        # Compute set(dirname(<platform_artifacts_dir>/aib_dir)).
        platform_artifacts_dirs = {paths.dirname(aib_dir): None for aib_dir in aib_directories}.keys()
        if len(platform_artifacts_dirs) > 1:
            fail(
                """The platform artifacts directory cannot be determined due to ambiguity: %s.
If it's not possible to limit the files passed into `files`, try specifying `directory` manually.""" % platform_artifacts_dirs,
            )
        root_dir = platform_artifacts_dirs[0]

    return [FuchsiaPlatformArtifactsInfo(
        root = root_dir,
        files = ctx.files.files,
    )]

fuchsia_platform_artifacts = rule(
    doc = """Wraps a directory of prebuilt platform artifacts.""",
    implementation = _fuchsia_platform_artifacts_impl,
    provides = [FuchsiaPlatformArtifactsInfo],
    attrs = {
        "directory": attr.label(
            doc = "The directory of prebuilt platform artifacts.",
            allow_single_file = True,
        ),
        "files": attr.label(
            doc = "A filegroup including all files of this prebuilt AIB.",
            mandatory = True,
            allow_files = True,
        ),
    },
)
