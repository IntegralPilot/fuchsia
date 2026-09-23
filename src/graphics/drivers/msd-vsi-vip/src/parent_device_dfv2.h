// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_VSI_VIP_SRC_PARENT_DEVICE_DFV2_H_
#define SRC_GRAPHICS_DRIVERS_MSD_VSI_VIP_SRC_PARENT_DEVICE_DFV2_H_

#include <lib/driver/incoming/cpp/namespace.h>

class ParentDeviceDfv2 {
 public:
  std::shared_ptr<fdf::Namespace> incoming_;
};
#endif  // SRC_GRAPHICS_DRIVERS_MSD_VSI_VIP_SRC_PARENT_DEVICE_DFV2_H_
