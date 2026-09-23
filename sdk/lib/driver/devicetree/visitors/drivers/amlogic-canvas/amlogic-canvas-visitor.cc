// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "amlogic-canvas-visitor.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>

namespace amlogic_canvas_dt {

zx::result<> AmlogicCanvasVisitor::AddChildNodeSpec(fdf_devicetree::Node& child) {
  std::vector bind_rules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
  };

  std::vector bind_properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
  };

  auto amlogic_canvas_node = fuchsia_driver_framework::ParentSpec2{{bind_rules, bind_properties}};

  child.AddNodeSpec(amlogic_canvas_node);
  fdf::debug("Added amlogic canvas node spec of to '{}'.", child.name());

  return zx::ok();
}

zx::result<> AmlogicCanvasVisitor::Visit(fdf_devicetree::Node& node,
                                         const devicetree::PropertyDecoder& decoder) {
  if (node.properties().find("amlogic,canvas") != node.properties().end()) {
    return AddChildNodeSpec(node);
  }
  return zx::ok();
}

}  // namespace amlogic_canvas_dt

REGISTER_DEVICETREE_VISITOR(amlogic_canvas_dt::AmlogicCanvasVisitor);
