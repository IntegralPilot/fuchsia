// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "lib/driver/devicetree/visitors/default/bind-property/bind-property.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

#include <bind/fuchsia/cpp/bind.h>

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace fdf_devicetree {

constexpr const char kCompatibleProp[] = "compatible";

zx::result<> BindPropertyVisitor::Visit(Node& node, const devicetree::PropertyDecoder& decoder) {
  auto compatible = node.GetProperty<std::vector<std::string>>(kCompatibleProp);
  if (compatible.is_error() && compatible.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::warn("Node has invalid compatible property. node_name: {}, status: {}", node.name(),
              compatible.status_value());

    return compatible.take_error();
  }

  if (!compatible.is_ok() || compatible->empty()) {
    fdf::debug("Node '{}' has no compatible property.", node.name());

    return zx::ok();
  }

  fdf::NodeProperty2 prop(bind_fuchsia::COMPATIBLE,
                          fdf::NodePropertyValue::WithStringValue(compatible->front()));
  node.AddBindProperty(std::move(prop));

  fdf::debug("Added property {} to node '{}'", compatible->front(), node.name());

  return zx::ok();
}

}  // namespace fdf_devicetree
