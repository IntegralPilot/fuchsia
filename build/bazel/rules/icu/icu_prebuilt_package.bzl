# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_icu_config//:constants.bzl",
    "icu_flavors",
)
load(
    "//build/bazel/rules/packages:prebuilt_package.bzl",
    "prebuilt_package",
)
load(
    ":icu_names.bzl",
    "icu_flavored_label",
    "icu_flavored_name",
)

def icu_prebuilt_package(*, name, archive, **kwargs):
    """Declares a prebuilt_package that comes in icu flavored versions as well as the standard.

    See prebuilt_package() for further documentation.
    """

    # The standard package:
    prebuilt_package(
        name = name,
        archive = archive,
        **kwargs
    )

    # Each of the icu flavors of the above package.
    for icu_flavor in icu_flavors:
        prebuilt_package(
            name = icu_flavored_name(name, icu_flavor),
            archive = icu_flavored_label(archive, icu_flavor),
            **kwargs
        )
