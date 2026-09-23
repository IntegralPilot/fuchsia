# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities used for workflows/tasks."""

load(
    "@fuchsia_rules_common//:utils.bzl",
    _flatten = "flatten",
)
load(
    "//fuchsia/private:utils.bzl",
    _alias = "alias",
    _collect_runfiles = "collect_runfiles",
    _normalized_target_name = "normalized_target_name",
    _rule_variants = "rule_variants",
    _wrap_executable = "wrap_executable",
)

alias = _alias
collect_runfiles = _collect_runfiles
flatten = _flatten
normalized_target_name = _normalized_target_name
rule_variants = _rule_variants
wrap_executable = _wrap_executable
