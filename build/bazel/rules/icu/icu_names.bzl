# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_icu_config//:constants.bzl", "icu_commits")

visibility([
    "//bundles/assembly/...",
    "//build/bazel/rules/icu/...",
])

def icu_flavored_name(name, icu_flavor):
    """Given a name, create an new name with a suffix containing the flavor and commit."""
    return "%s.icu_%s_%s" % (name, icu_flavor, icu_commits[icu_flavor])

def icu_flavored_label(label, icu_flavor):
    """Given a label, create an new label with a suffix containing the flavor and commit."""
    label = Label(label)
    name = label.name

    return label.same_package_label(icu_flavored_name(name, icu_flavor))
