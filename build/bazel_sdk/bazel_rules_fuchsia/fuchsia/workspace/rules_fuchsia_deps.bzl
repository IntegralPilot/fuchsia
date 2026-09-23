# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Loads external repositores needed by @rules_fuchsia."""

load("@bazel_tools//tools/build_defs/repo:http.bzl", "http_archive")
load("@bazel_tools//tools/build_defs/repo:utils.bzl", "maybe")
load("@platforms//host:extension.bzl", "host_platform_repo")
load(
    "//fuchsia/workspace:check_bazel_version.bzl",
    "assert_bazel_version",
)

# buildifier: disable=function-docstring
def rules_fuchsia_deps():
    assert_bazel_version(min = "6.0.0")

    maybe(
        name = "rules_python",
        repo_rule = http_archive,
        sha256 = "a3a6e99f497be089f81ec082882e40246bfd435f52f4e82f37e89449b04573f6",
        strip_prefix = "rules_python-0.10.2",
        url = "https://github.com/bazelbuild/rules_python/archive/refs/tags/0.10.2.tar.gz",
    )

    maybe(
        name = "rules_license",
        repo_rule = http_archive,
        sha256 = "4531deccb913639c30e5c7512a054d5d875698daeb75d8cf90f284375fe7c360",
        url = "https://github.com/bazelbuild/rules_license/releases/download/0.0.7/rules_license-0.0.7.tar.gz",
    )
    # rules_license_dependencies needs to be loaded from @rules_license//:deps.bzl
    # and invoked here, but that is not possible. Fortunately all it does is fetch
    # rules_python which we do above. But this may become a problem in the future.

    # Ensures @platforms is 0.0.6, which starts to include fuchsia.
    maybe(
        name = "platforms",
        repo_rule = http_archive,
        sha256 = "5308fc1d8865406a49427ba24a9ab53087f17f5266a7aabbfc28823f3916e1ca",
        url = "https://github.com/bazelbuild/platforms/releases/download/0.0.6/platforms-0.0.6.tar.gz",
    )

    maybe(
        name = "host_platform",
        repo_rule = host_platform_repo,
    )
