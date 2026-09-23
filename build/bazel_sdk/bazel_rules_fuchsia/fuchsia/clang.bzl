# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public definitions for Fuchsia clang workspace rules"""

load(
    "//fuchsia/workspace:fuchsia_clang_repository.bzl",
    _fuchsia_clang_ext = "fuchsia_clang_ext",
    _fuchsia_clang_repository = "fuchsia_clang_repository",
)

fuchsia_clang_repository = _fuchsia_clang_repository
fuchsia_clang_ext = _fuchsia_clang_ext
