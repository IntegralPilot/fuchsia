# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Private definitions for Fuchsia rules needed in the `fuchsia-infra-bazel-rules` repository.

Nothing should use this file except for existing `load()` statements in `fuchsia-infra-bazel-rules`.

TODO(https://fxbug.dev/553008868): Remove this file once `fuchsia-infra-bazel-rules` has been
updated to use a more specific "API" for these symbols.
"""

load(
    "@fuchsia_rules_common//debug_symbols:providers.bzl",
    _FuchsiaDebugSymbolInfo = "FuchsiaDebugSymbolInfo",
)
load("@fuchsia_rules_common//packages:providers.bzl", _FuchsiaPackageInfo = "FuchsiaPackageInfo")
load(
    "//fuchsia/private:providers.bzl",
    _FuchsiaProductBundleInfo = "FuchsiaProductBundleInfo",
    _FuchsiaRunnableInfo = "FuchsiaRunnableInfo",
)
load(
    "//fuchsia/private:utils.bzl",
    _collect_runfiles = "collect_runfiles",
    _wrap_executable = "wrap_executable",
)
load(
    "//fuchsia/private/assembly:providers.bzl",
    _FuchsiaBoardConfigInfo = "FuchsiaBoardConfigInfo",
    _FuchsiaBoardInputBundleInfo = "FuchsiaBoardInputBundleInfo",
    _FuchsiaBoardInputBundleSetInfo = "FuchsiaBoardInputBundleSetInfo",
)

# bazel_rules_fuchsia/fuchsia/private/assembly/providers.bzl
FuchsiaBoardConfigInfo = _FuchsiaBoardConfigInfo
FuchsiaBoardInputBundleInfo = _FuchsiaBoardInputBundleInfo
FuchsiaBoardInputBundleSetInfo = _FuchsiaBoardInputBundleSetInfo

# fuchsia_rules_common/debug_symbols/providers.bzl
FuchsiaDebugSymbolInfo = _FuchsiaDebugSymbolInfo

# fuchsia_rules_common/packages/providers.bzl
FuchsiaPackageInfo = _FuchsiaPackageInfo

# bazel_rules_fuchsia/fuchsia/private/providers.bzl
FuchsiaProductBundleInfo = _FuchsiaProductBundleInfo
FuchsiaRunnableInfo = _FuchsiaRunnableInfo

# bazel_rules_fuchsia/fuchsia/private/utils.bzl
collect_runfiles = _collect_runfiles
wrap_executable = _wrap_executable
