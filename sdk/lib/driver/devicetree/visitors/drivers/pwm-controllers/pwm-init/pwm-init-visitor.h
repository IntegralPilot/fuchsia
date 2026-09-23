// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_INIT_PWM_INIT_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_INIT_PWM_INIT_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>

namespace pwm_init_visitor_dt {

class PwmInitVisitor : public fdf_devicetree::Visitor {
 public:
  PwmInitVisitor() = default;
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child);
};

}  // namespace pwm_init_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_PWM_CONTROLLERS_PWM_INIT_PWM_INIT_VISITOR_H_
