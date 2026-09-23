# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""The fuchsia_idk_repository() repository rule definition."""

load(
    "@rules_fuchsia//fuchsia/workspace:utils.bzl",
    "abspath_from_full_label_or_repo_relpath",
)

# This repository rule is used to populate an IDK export directory
# whose metadata file reference Ninja-generated artifacts as targets,
# for example for `sdk://pkg/fdio/meta.json` the original metadata
# file looks like:
#
# ```
# {
#   "binaries": {
#     "x64": {
#       "debug": ".build-id/87/e97ba9fa4548c0060e2cb43b2537a87c62fae4.debug",
#       "dist": "arch/x64/dist/libfdio.so",
#       "dist_path": "lib/libfdio.so",
#       "link": "arch/x64/lib/libfdio.so"
#     }
#   },
#   ...
# }
# ```
#
# But the one found in this repository will contain:
#
# ```
# {
#   "binaries": {
#     "x64": {
#       "debug": "@fuchsia_idk//arch/x64:debug/libfdio.so",
#       "dist": "@fuchsia_idk//arch/x64:dist/libfdio.so",
#       "dist_path": "lib/libfdio.so",
#       "link": "@fuchsia_idk//arch/x64:lib/libfdio.so"
#     }
#   },
#   ...
# }
# ```
#
# Where the labels point either to symlinks to the source IDK
# export directory, or even Bazel targets that build them on demand
# if needed.
#
def _fuchsia_idk_repository(repo_ctx):
    """Create a fuchsia_idk_repository() repository."""
    if hasattr(repo_ctx.attr, "content_hash_file"):
        repo_ctx.path(Label(repo_ctx.attr.content_hash_file))

    idk_export_dir_path = repo_ctx.path("%s/%s" % (repo_ctx.workspace_root, repo_ctx.attr.idk_export_dir))
    ninja_build_dir_path = repo_ctx.path("%s/%s" % (repo_ctx.workspace_root, repo_ctx.attr.ninja_build_dir))
    ret = repo_ctx.execute(
        [
            abspath_from_full_label_or_repo_relpath(
                repo_ctx,
                repo_ctx.attr.python_executable,
            ),
            "-S",
            repo_ctx.path(Label("@//build/bazel/fuchsia_idk:generate_repository.py")),
            "--repository-name",
            repo_ctx.name,
            "--output-dir",
            ".",
            "--input-dir",
            idk_export_dir_path,
            "--ninja-build-dir",
            ninja_build_dir_path,
        ],
        quiet = False,
    )
    if ret.return_code != 0:
        fail("Could not generate fuchsia IDK repository: %s" % ret.stderr)

    repo_ctx.file("WORKSPACE.bazel", "workspace(name = \"{name}\")\n".format(name = repo_ctx.name))
    repo_ctx.file("MODULE.bazel", "module(name = \"{name}\", version = \"1\")\n".format(name = repo_ctx.name))

    #repo_ctx.symlink(repo_ctx.path(Label("@//build/bazel/fuchsia_idk:repository.BUILD.bazel")), "BUILD.bazel")
    repo_ctx.symlink(idk_export_dir_path, "ninja_idk_export_dir_symlink")

fuchsia_idk_repository = repository_rule(
    implementation = _fuchsia_idk_repository,
    doc = "Generate a repository exposing an IDK export directory with Bazel targets.",
    attrs = {
        "idk_export_dir": attr.string(
            doc = "Path to input IDK export directory, relative to workspace root.",
            mandatory = True,
        ),
        "ninja_build_dir": attr.string(
            doc = "Path to Ninja build directory, relative to workspace root.",
            mandatory = True,
        ),
        "python_executable": attr.string(
            doc = "Path to python3 executable, a full label, or relative to workspace root.",
            mandatory = True,
        ),
        "content_hash_file": attr.string(
            doc = "Path to content hash file for this repository, a full label.",
            mandatory = False,
        ),
    },
)
