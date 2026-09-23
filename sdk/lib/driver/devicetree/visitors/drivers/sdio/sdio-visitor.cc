// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdio-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/sdio/cpp/bind.h>

namespace sdio_dt {

zx::result<> SdioVisitor::Visit(fdf_devicetree::Node& node,
                                const devicetree::PropertyDecoder& decoder) {
  return zx::ok();
}

zx::result<> SdioVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (!is_match(node)) {
    return zx::ok();
  }

  for (auto& child : node.children()) {
    if (zx::result<> result = ParseChild(node, child); result.is_error()) {
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<> SdioVisitor::ParseChild(fdf_devicetree::Node& parent,
                                     fdf_devicetree::ChildNode& child) {
  auto status = child.GetProperty<std::string>("status");
  if (status.is_ok() && *status != "okay") {
    fdf::debug("SDIO child '{}' is disabled.", child.name());
    return zx::ok();
  }

  auto reg = child.GetProperty<std::vector<uint32_t>>("reg");
  if (reg.is_error()) {
    // Ignore config child nodes.
    if (child.name() == "fuchsia,config") {
      return zx::ok();
    }

    fdf::error("SDIO child '{}' has no reg property: {}", child.name(), reg.status_string());
    return reg.take_error();
  }

  for (uint32_t func : *reg) {
    if (func == 0) {
      fdf::info("SDIO visitor: Skipping function 0 parent spec addition for node '{}'",
                child.name());
      continue;
    }
    auto sdio_parent = fuchsia_driver_framework::ParentSpec2{
        {.bind_rules =
             {
                 fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                                         bind_fuchsia_sdio::BIND_PROTOCOL_DEVICE),
                 fdf::MakeAcceptBindRule(bind_fuchsia::SDIO_FUNCTION, func),
             },
         .properties = {
             fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.sdio.Service"),
             fdf::MakeProperty2(bind_fuchsia::SDIO_FUNCTION, func),
         }}};
    child.AddNodeSpec(sdio_parent);
    fdf::info("SDIO visitor: Added SDIO parent spec (func {}) to node '{}'", func, child.name());
  }

  return zx::ok();
}

bool SdioVisitor::is_match(fdf_devicetree::Node& node) {
  // Match if the node is an SDMMC host controller.
  return node.name().find("mmc@") == 0 || node.name().find("sdhci@") == 0;
}

}  // namespace sdio_dt

REGISTER_DEVICETREE_VISITOR(sdio_dt::SdioVisitor);
