# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helper rules for the tests in this package."""

load(
    "//build/bazel/rules:stamp_group.bzl",
    "STAMP_GROUP_NON_DEPS_ATTRS",
    "stamp_group_impl",
)

visibility(["//build/bazel/bazel_idk/tests/..."])

build_for_host = rule(
    doc = "Builds `deps` using the host configuration.",
    implementation = stamp_group_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "List of Labels to build using the host configuration.",
            mandatory = True,
            cfg = "exec",
        ),
    } | STAMP_GROUP_NON_DEPS_ATTRS,
)
