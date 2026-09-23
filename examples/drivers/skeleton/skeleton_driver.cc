// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "skeleton_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>

namespace skeleton {

SkeletonDriver::SkeletonDriver() : DriverBase2("skeleton_driver") {}

zx::result<> SkeletonDriver::Start(fdf::DriverContext context) {
  // Instructions: Put driver initialization logic in this function, such as adding children
  // and setting up client-server transport connections.
  // If the initialization logic is asynchronous, prefer to override
  // DriverBase2::Start(fdf::DriverContext context, fdf::StartCompleter completer) over this
  // function.
  return zx::ok();
}

}  // namespace skeleton

FUCHSIA_DRIVER_EXPORT2(skeleton::SkeletonDriver);
