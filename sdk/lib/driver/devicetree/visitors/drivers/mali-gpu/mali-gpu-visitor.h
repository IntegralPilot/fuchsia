// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MALI_GPU_MALI_GPU_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MALI_GPU_MALI_GPU_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>

namespace mali_gpu_dt {

class MaliGpuVisitor : public fdf_devicetree::Visitor {
 public:
  MaliGpuVisitor() = default;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child);
};

}  // namespace mali_gpu_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_MALI_GPU_MALI_GPU_VISITOR_H_
