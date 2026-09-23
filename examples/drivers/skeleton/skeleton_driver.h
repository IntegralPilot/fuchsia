// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_SKELETON_SKELETON_DRIVER_H_
#define EXAMPLES_DRIVERS_SKELETON_SKELETON_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>

namespace skeleton {

class SkeletonDriver : public fdf::DriverBase2 {
 public:
  SkeletonDriver();

  // Called by the driver framework to initialize the driver instance.
  zx::result<> Start(fdf::DriverContext context) override;
};

}  // namespace skeleton

#endif  // EXAMPLES_DRIVERS_SKELETON_SKELETON_DRIVER_H_
