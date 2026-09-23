// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "lib/driver/devicetree/visitors/default/driver-host/driver-host.h"

#include <lib/devicetree/devicetree.h>
#include <lib/driver/logging/cpp/logger.h>

#include <optional>

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace fdf_devicetree {

constexpr const char kDriverHostProp[] = "fuchsia,driver-host";

zx::result<> DriverHostVisitor::Visit(Node& node, const devicetree::PropertyDecoder& decoder) {
  auto driver_host_prop = node.properties().find(kDriverHostProp);
  if (driver_host_prop == node.properties().end()) {
    return zx::ok();
  }

  std::optional driver_host = driver_host_prop->second.AsString();
  if (!driver_host.has_value()) {
    fdf::error("Driver Host property for node '{}' is not a valid string.", node.name());
    return zx::ok();
  }

  fdf::debug("Driver Host ({}) added to node '{}'.", driver_host.value(), node.name());
  node.SetDriverHost(driver_host.value());

  return zx::ok();
}

}  // namespace fdf_devicetree
