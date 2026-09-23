# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Forwarding definitions for IDK providers."""

load(
    "//build/bazel/rules/idk/private:providers.bzl",
    _FuchsiaIdkAtomInfo = "FuchsiaIdkAtomInfo",
    _FuchsiaIdkMoleculeInfo = "FuchsiaIdkMoleculeInfo",
)

FuchsiaIdkAtomInfo = _FuchsiaIdkAtomInfo
FuchsiaIdkMoleculeInfo = _FuchsiaIdkMoleculeInfo
