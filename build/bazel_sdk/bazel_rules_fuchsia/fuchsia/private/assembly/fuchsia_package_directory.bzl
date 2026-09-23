# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule to make a Fuchsia package directory available during build."""

load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")

def _fuchsia_package_directory_impl(ctx):
    return [
        FuchsiaPackageInfo(
            package_manifest = ctx.file.package_manifest,
            files = ctx.files.files,
        ),
    ]

_fuchsia_package = rule(
    doc = "Wraps information of a package in a FuchsiaPackageInfo provider.",
    implementation = _fuchsia_package_directory_impl,
    provides = [FuchsiaPackageInfo],
    attrs = {
        "package_manifest": attr.label(
            doc = "Manifest of this package.",
            allow_single_file = True,
            mandatory = True,
        ),
        "files": attr.label_list(
            doc = "All files belong to this package. This list can include redundant files (for example, from a glob), as long as all files referenced by the package manifest are included.",
            default = [],
        ),
        # NOTE: Support for drivers are intentionally left out because they are
        # not used anywhere, so there is no reliable way to test. They can be
        # added when there are users.
    },
)

# **kwargs is not documented.
# buildifier: disable=function-docstring-args
def fuchsia_package_directory(*, name, root, package_manifest, **kwargs):
    """Creates a Fuchsia package from a directory.

    This enables Bazel assembly to consume packages distributed in the form of
    directories.

    NOTE: Packages are defined by their manifests, and it is possible for
    multiple packages to share the same directory.

    Usages example:

        fuchsia_package_directory(
          name = "foo",
          package_manifest = "packages/foo/package_manifest.json",
          root = "path/to/root",
        )

        fuchsia_package_directory(
          name = "bar",
          package_manifest = "packages/bar/package_manifest.json",
          root = "path/to/root",
        )

    Args:
      name: Name of this target.
      root: Path to directory root.
      package_manifest: Path to package manifest, relative to root.
    """
    _all_files_target = "{}_all_files".format(name)
    native.filegroup(
        name = _all_files_target,
        srcs = native.glob(["{}/**/*".format(root)]),
    )
    _fuchsia_package(
        name = name,
        package_manifest = root + "/" + package_manifest,
        files = [":{}".format(_all_files_target)],
        **kwargs
    )
