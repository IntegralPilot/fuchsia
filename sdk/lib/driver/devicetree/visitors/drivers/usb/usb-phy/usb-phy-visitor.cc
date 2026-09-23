// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-phy-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/designware/platform/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/usb/phy/cpp/bind.h>

namespace usb_phy_visitor_dt {

UsbPhyVisitor::UsbPhyVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kPhys, kPhyCells, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kPhyNames, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

bool UsbPhyVisitor::is_match(const std::string& name) {
  return name.find("usb-phy") != std::string::npos ||  //
         name.find("usb2") != std::string::npos ||     //
         name.find("usb3") != std::string::npos;
}

zx::result<> UsbPhyVisitor::Visit(fdf_devicetree::Node& node,
                                  const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Usb visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  auto phys = parser_output->Get<fdf_devicetree::References>(kPhys);
  if (phys) {
    auto phy_names = parser_output->Get<std::vector<std::string>>(kPhyNames);
    if (!phy_names) {
      if (phys->size() > 1) {
        fdf::error("Node '{}' is missing phy-names.", node.name());

        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    } else {
      if (phy_names->size() != phys->size()) {
        fdf::error(
            "Node '{}' does not have required number of phy-names. Expected ({}), actual ({}).",
            node.name(), phys->size(), phy_names->size());

        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }

    for (uint32_t index = 0; index < phys->size(); index++) {
      auto& reference = (*phys)[index];
      if (!reference.reference_node()) {
        fdf::error("Node '{}' has invalid phy reference at {} index.", node.name(), index);

        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      if (!is_match(reference.reference_node().name())) {
        // This reference is unlikely to be for a usb phy
        continue;
      }

      auto result = AddChildNodeSpec(node, phy_names ? (*phy_names)[index] : "");
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }
  return zx::ok();
}

zx::result<> UsbPhyVisitor::AddChildNodeSpec(fdf_devicetree::Node& child,
                                             std::string_view phy_name) {
  std::vector bind_rules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
  };
  std::vector bind_properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
  };

  std::optional<uint32_t> did;
  if (phy_name == "xhci-phy" || phy_name == "xhci") {
    did = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_XHCI;
  } else if (phy_name == "dwc2-phy") {
    did = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_USB_DWC2;
  } else if (phy_name == "dwc3-phy") {
    did = bind_fuchsia_designware_platform::BIND_PLATFORM_DEV_DID_DWC3;
  } else if (!phy_name.empty()) {
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(bind_fuchsia_usb_phy::NAME, phy_name));
    bind_properties.emplace_back(fdf::MakeProperty2(bind_fuchsia_usb_phy::NAME, phy_name));
  }

  if (did) {
    std::string_view parent_name = phy_name;
    if (phy_name == "xhci-phy") {
      parent_name = "xhci";
    }
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(
        bind_fuchsia::PLATFORM_DEV_VID, bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC));
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(
        bind_fuchsia::PLATFORM_DEV_PID, bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC));
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID, *did));

    bind_properties.emplace_back(fdf::MakeProperty2(bind_fuchsia::NAME, parent_name));
    bind_properties.emplace_back(fdf::MakeProperty2(
        bind_fuchsia::PLATFORM_DEV_VID, bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC));
    bind_properties.emplace_back(fdf::MakeProperty2(
        bind_fuchsia::PLATFORM_DEV_PID, bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC));
    bind_properties.emplace_back(fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID, *did));
  }

  auto phy_node = fuchsia_driver_framework::ParentSpec2{{bind_rules, bind_properties}};

  child.AddNodeSpec(phy_node);

  fdf::debug("Added '{}' bind rules to node '{}'.", phy_name, child.name());

  return zx::ok();
}

}  // namespace usb_phy_visitor_dt

REGISTER_DEVICETREE_VISITOR(usb_phy_visitor_dt::UsbPhyVisitor);
