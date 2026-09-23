// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pwm-init-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pwm/cpp/bind.h>

namespace pwm_init_visitor_dt {

zx::result<> PwmInitVisitor::AddChildNodeSpec(fdf_devicetree::Node& child) {
  std::vector bind_rules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM)};

  std::vector bind_properties = {
      fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_pwm::BIND_INIT_STEP_PWM)};

  auto pwm_init_node = fuchsia_driver_framework::ParentSpec2{{bind_rules, bind_properties}};

  child.AddNodeSpec(pwm_init_node);
  fdf::debug("Added pwm init node spec of to '{}'.", child.name());

  return zx::ok();
}

zx::result<> PwmInitVisitor::Visit(fdf_devicetree::Node& node,
                                   const devicetree::PropertyDecoder& decoder) {
  if (node.properties().find("pwm-init") != node.properties().end()) {
    return AddChildNodeSpec(node);
  }
  return zx::ok();
}

}  // namespace pwm_init_visitor_dt

REGISTER_DEVICETREE_VISITOR(pwm_init_visitor_dt::PwmInitVisitor);
