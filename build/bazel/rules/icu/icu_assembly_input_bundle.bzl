# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_icu_config//:constants.bzl",
    "icu_flavors",
)
load(
    "//build/bazel/rules/assembly:assembly_input_bundle.bzl",
    "assembly_input_bundle",
)
load(
    ":icu_names.bzl",
    "icu_flavored_label",
    "icu_flavored_name",
)

def icu_assembly_input_bundle(
        *,
        name,
        icu_base_packages = [],
        icu_cache_packages = [],
        **kwargs):
    """Creates a set of Assembly Input Bundles, one for each ICU flavor, and one standard one.

    It takes all the arguments of the 'assembly_input_bundle()' macro, with the addition of
    the following.

    Args:
        name: The name of the target.

        icu_base_packages: [list of labels] Package targets to include in the base package set,
            which come in icu flavors as well as the standard target, and should each be included
            in the AIBs corresponding to the same ICU flavor.

        icu_cache_packages: [list of labels] Same as icu_base_package, but for the cache package
            set.
        """

    base_packages = kwargs.get("base_packages", [])
    cache_packages = kwargs.get("cache_packages", [])

    icu_kwargs = {
        key: value
        for key, value in kwargs.items()
        if key not in ["base_packages", "cache_packages"]
    }

    # The "standard" version of the AIB.
    assembly_input_bundle(
        name = name,
        base_packages = base_packages + icu_base_packages,
        cache_packages = cache_packages + icu_cache_packages,
        **icu_kwargs
    )

    for icu_flavor in icu_flavors:
        assembly_input_bundle(
            name = icu_flavored_name(name, icu_flavor),
            base_packages = base_packages + [
                icu_flavored_label(name, icu_flavor)
                for name in icu_base_packages
            ],
            cache_packages = cache_packages + [
                icu_flavored_label(name, icu_flavor)
                for name in icu_cache_packages
            ],
            **icu_kwargs
        )
