// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test.h"

#include <lib/driver/component/cpp/driver_export2.h>

zx::result<> TestDriver::Start(fdf::DriverContext context) {
  fuchsia_driver_framework::DevfsAddArgs devfs_args(
      {.connector_supports{fuchsia_device_fs::ConnectionType::kController}});

  // Create a child that driver-test-realm tests can watch for.
  zx::result child = AddOwnedChild("test2", devfs_args);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

FUCHSIA_DRIVER_EXPORT2(TestDriver);
