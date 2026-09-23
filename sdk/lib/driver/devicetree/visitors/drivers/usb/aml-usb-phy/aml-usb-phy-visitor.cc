// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-usb-phy-visitor.h"

#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>
#include <vector>

#include <usb/usb.h>

namespace aml_usb_phy_visitor_dt {

AmlUsbPhyVisitor::AmlUsbPhyVisitor()
    : fdf_devicetree::DriverVisitor({"amlogic,g12a-usb-phy", "amlogic,g12b-usb-phy"}) {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kDrModes, /* required */ true));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kRegNames, /* required */ true));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kCompatible, /* required */ true));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> AmlUsbPhyVisitor::DriverVisit(fdf_devicetree::Node& node,
                                           const devicetree::PropertyDecoder& decoder) {
  auto parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Aml usb phy visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  fuchsia_hardware_usb_phy::AmlogicPhyType phy_type;
  auto compatible = parser_output->Get<std::vector<std::string>>(kCompatible);
  if ((*compatible)[0] == "amlogic,g12a-usb-phy") {
    phy_type = fuchsia_hardware_usb_phy::AmlogicPhyType::kG12A;
  } else if ((*compatible)[0] == "amlogic,g12b-usb-phy") {
    phy_type = fuchsia_hardware_usb_phy::AmlogicPhyType::kG12B;
  } else {
    fdf::error("Node '{}' has invalid compatible string. Cannot determine PHY type. ", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto reg_names = parser_output->Get<std::vector<std::string>>(kRegNames);
  auto dr_modes = parser_output->Get<std::vector<std::string>>(kDrModes);

  if (reg_names->size() - 1 != dr_modes->size()) {
    fdf::error(
        "Node '{}' does not have entries in dr_modes for each PHY device. Expected - {}, Actual - {}.",
        node.name(), reg_names->size() - 1, dr_modes->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  uint32_t reg_name_index = 1;
  std::vector<fuchsia_hardware_usb_phy::UsbPhyMode> phy_modes;
  for (auto& mode : *dr_modes) {
    fuchsia_hardware_usb_phy::UsbPhyMode phy_mode{};
    if (mode == "host") {
      phy_mode.dr_mode() = fuchsia_hardware_usb_phy::Mode::kHost;
    } else if (mode == "peripheral") {
      phy_mode.dr_mode() = fuchsia_hardware_usb_phy::Mode::kPeripheral;
    } else if (mode == "otg") {
      phy_mode.dr_mode() = fuchsia_hardware_usb_phy::Mode::kOtg;
    }

    auto& phy_name = (*reg_names)[reg_name_index++];
    if (phy_name == "usb2-phy") {
      phy_mode.protocol() = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20;
      phy_mode.is_otg_capable() = false;
    } else if (phy_name == "usb2-otg-phy") {
      phy_mode.protocol() = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20;
      phy_mode.is_otg_capable() = true;
    } else if (phy_name == "usb3-phy") {
      phy_mode.protocol() = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb30;
      phy_mode.is_otg_capable() = false;
    }

    phy_modes.emplace_back(phy_mode);
  }

  fuchsia_hardware_usb_phy::Metadata metadata{
      {.usb_phy_modes{std::move(phy_modes)}, .phy_type = phy_type}};
  fit::result persisted_metadata = fidl::Persist(metadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to persist metadata for node {}: {}", node.name(),
               persisted_metadata.error_value().FormatDescription());

    return zx::error(persisted_metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata usb_phy_metadata = {{
      .id = fuchsia_hardware_usb_phy::Metadata::kSerializableName,
      .data = std::move(persisted_metadata.value()),
  }};
  node.AddMetadata(std::move(usb_phy_metadata));
  fdf::debug("Added {} usb phy metadata to node '{}'.", phy_modes.size(), node.name());

  return zx::ok();
}

}  // namespace aml_usb_phy_visitor_dt

REGISTER_DEVICETREE_VISITOR(aml_usb_phy_visitor_dt::AmlUsbPhyVisitor);
