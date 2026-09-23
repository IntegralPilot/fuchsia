# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public definitions for Fuchsia companion image workspace rules"""

load(
    "//fuchsia/workspace:fuchsia_products_repository.bzl",
    _fuchsia_products_repository = "fuchsia_products_repository",
)

fuchsia_products_repository = _fuchsia_products_repository
