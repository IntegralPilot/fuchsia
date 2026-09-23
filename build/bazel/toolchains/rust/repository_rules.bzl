# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Repository rules used to populate Rust-based repositories."""

load(
    "@rules_fuchsia//fuchsia/workspace:utils.bzl",
    "abspath_from_full_label_or_repo_relpath",
)

def _generate_prebuilt_rust_toolchain_repository_impl(repo_ctx):
    # Symlink the content of the Rust installation directory into the repository.
    # This allows us to add Bazel-specific files in this location.
    repo_ctx.execute(
        [
            repo_ctx.path(Label("@//build/bazel/scripts:symlink-directory.py")),
            abspath_from_full_label_or_repo_relpath(
                repo_ctx,
                repo_ctx.attr.rust_install_dir,
            ),
            ".",
        ],
        quiet = False,  # False for debugging!
    )

    # Symlink the BUILD.bazel file.
    repo_ctx.symlink(
        repo_ctx.workspace_root.get_child("build/bazel/toolchains/rust/rust.BUILD.bazel"),
        "BUILD.bazel",
    )

generate_prebuilt_rust_toolchain_repository = repository_rule(
    implementation = _generate_prebuilt_rust_toolchain_repository_impl,
    attrs = {
        "rust_install_dir": attr.string(
            mandatory = True,
            doc = "Location of prebuilt Rust toolchain installation, a full label, or relative to workspace dir",
        ),
    },
)
