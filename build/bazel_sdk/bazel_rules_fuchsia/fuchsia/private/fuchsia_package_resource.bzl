# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//packages:resources.bzl",
    _fuchsia_package_resource_collection = "fuchsia_package_resource_collection",
    _fuchsia_package_resource_group = "fuchsia_package_resource_group",
)

fuchsia_package_resource_collection = _fuchsia_package_resource_collection
fuchsia_package_resource_group = _fuchsia_package_resource_group
