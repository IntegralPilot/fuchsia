// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dwc2-visitor.h"

#include <fidl/fuchsia.hardware.usb.dwc2/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>

namespace dwc2_visitor_dt {

Dwc2Visitor::Dwc2Visitor() : fdf_devicetree::DriverVisitor({"snps,dwc2"}) {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kGRxFifoSize, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kGNpTxFifoSize, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32ArrayProperty>(kGTxFifoSize, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kGTurnaroundTime, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kDmaBurstLen, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> Dwc2Visitor::DriverVisit(fdf_devicetree::Node& node,
                                      const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("dwc2 visitor parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  fuchsia_hardware_usb_dwc2::Metadata dwc2_metadata = {};
  bool metadata_found = false;

  if (auto value = parser_output->Get<uint32_t>(kGRxFifoSize)) {
    dwc2_metadata.rx_fifo_size() = *value;
    metadata_found = true;
  }

  if (auto value = parser_output->Get<uint32_t>(kGNpTxFifoSize)) {
    dwc2_metadata.nptx_fifo_size() = *value;
    metadata_found = true;
  }

  if (auto value = parser_output->Get<uint32_t>(kGTurnaroundTime)) {
    dwc2_metadata.usb_turnaround_time() = *value;
    metadata_found = true;
  }

  if (auto value = parser_output->Get<uint32_t>(kDmaBurstLen)) {
    dwc2_metadata.dma_burst_len() = static_cast<fuchsia_hardware_usb_dwc2::DmaBurstLen>(*value);
    metadata_found = true;
  }

  if (auto tx_fifo_sizes = parser_output->Get<std::vector<uint32_t>>(kGTxFifoSize)) {
    if (tx_fifo_sizes->size() > dwc2_metadata.tx_fifo_sizes().size()) {
      fdf::error("Node '{}' has invalid '{}'. Expected size to be <= {}, actual: {}.", node.name(),
                 kGTxFifoSize, dwc2_metadata.tx_fifo_sizes().size(), tx_fifo_sizes->size());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    for (size_t i = 0; i < tx_fifo_sizes->size(); ++i) {
      dwc2_metadata.tx_fifo_sizes()[i] = (*tx_fifo_sizes)[i];
    }
    metadata_found = true;
  }

  if (metadata_found) {
    fit::result persisted_metadata = fidl::Persist(dwc2_metadata);
    if (!persisted_metadata.is_ok()) {
      fdf::error("Failed to persist dwc2 metadata: {}",
                 persisted_metadata.error_value().FormatDescription());

      return zx::error(persisted_metadata.error_value().status());
    }

    fuchsia_hardware_platform_bus::Metadata metadata({
        .id = fuchsia_hardware_usb_dwc2::Metadata::kSerializableName,
        .data = std::move(persisted_metadata.value()),
    });
    node.AddMetadata(std::move(metadata));
    fdf::debug("Added dwc2 metadata to node '{}'.", node.name());
  }

  return zx::ok();
}

}  // namespace dwc2_visitor_dt

REGISTER_DEVICETREE_VISITOR(dwc2_visitor_dt::Dwc2Visitor);
