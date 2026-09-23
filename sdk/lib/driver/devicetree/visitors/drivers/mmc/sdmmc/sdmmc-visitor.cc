// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-visitor.h"

#include <fidl/fuchsia.hardware.sdmmc/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <bind/fuchsia/cpp/bind.h>

#include "lib/driver/devicetree/manager/visitor.h"

namespace sdmmc_dt {

SdmmcVisitor::SdmmcVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kMaxFrequency, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kNonRemovable, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kNoMmcHs400, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kNoMmcHs200, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kNoMmcHsDdr, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::BoolProperty>(kUseFidl, /* required */ false));
  sdmmc_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

bool SdmmcVisitor::is_match(std::string_view name) {
  // Check that the name begins with mmc@ or sdhci@.
  return name.find("mmc@") == 0 || name.find("sdhci@") == 0;
}

zx::result<> SdmmcVisitor::Visit(fdf_devicetree::Node& node,
                                 const devicetree::PropertyDecoder& decoder) {
  if (!is_match(node.name())) {
    return zx::ok();
  }

  zx::result parser_output = sdmmc_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("SDMMC visitor failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  fuchsia_hardware_sdmmc::SdmmcMetadata sdmmc_metadata = {};
  sdmmc_metadata.instance_identifier() = node.name();

  if (auto max_frequency = parser_output->Get<uint32_t>(kMaxFrequency)) {
    sdmmc_metadata.max_frequency() = *max_frequency;
  }

  sdmmc_metadata.removable() = !parser_output->Get<bool>(kNonRemovable);

  uint64_t host_prefs = 0;
  if (parser_output->Get<bool>(kNoMmcHs400)) {
    host_prefs |= static_cast<uint64_t>(fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs400);
  }
  if (parser_output->Get<bool>(kNoMmcHs200)) {
    host_prefs |= static_cast<uint64_t>(fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHs200);
  }
  if (parser_output->Get<bool>(kNoMmcHsDdr)) {
    host_prefs |= static_cast<uint64_t>(fuchsia_hardware_sdmmc::SdmmcHostPrefs::kDisableHsddr);
  }

  if (host_prefs) {
    sdmmc_metadata.speed_capabilities() =
        std::optional<fuchsia_hardware_sdmmc::SdmmcHostPrefs>(host_prefs);
  }

  sdmmc_metadata.use_fidl() = parser_output->Get<bool>(kUseFidl);

  fit::result encoded_metadata = fidl::Persist(sdmmc_metadata);
  if (!encoded_metadata.is_ok()) {
    fdf::error("Failed to encode SDMMC metadata for node {}: {}", node.name(),
               encoded_metadata.error_value().FormatDescription());

    return zx::error(encoded_metadata.error_value().status());
  }

  node.AddMetadata({{.id = fuchsia_hardware_sdmmc::SdmmcMetadata::kSerializableName,
                     .data = encoded_metadata.value()}});
  fdf::debug("SDMMC metadata added to node '{}'", node.name());

  return zx::ok();
}

}  // namespace sdmmc_dt

REGISTER_DEVICETREE_VISITOR(sdmmc_dt::SdmmcVisitor);
