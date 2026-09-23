# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Internal definitions for extending Fuchsia SDK rules in google3.

This file should be limited to providers and rule helper functions for
performance reasons (see go/widely-loaded-bzl-files)."""

load(
    "//fuchsia/private:providers.bzl",
    _FuchsiaFidlLibraryInfo = "FuchsiaFidlLibraryInfo",
)

FuchsiaFidlLibraryInfo = _FuchsiaFidlLibraryInfo
