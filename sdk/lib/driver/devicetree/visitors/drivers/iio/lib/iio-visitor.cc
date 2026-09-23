// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "iio-visitor.h"

#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>

namespace iio_dt {

IioVisitor::IioVisitor() {
  fdf_devicetree::Properties iio_properties = {};
  iio_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kIioChannelReference, kIioChannelCells, /* required */ false));
  iio_properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kIioChannelNames));
  iio_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(iio_properties));
}

bool IioVisitor::is_match(fdf_devicetree::Node& node) {
  auto channel_cells = node.properties().find(kIioChannelCells);
  return !(channel_cells == node.properties().end());
}

zx::result<> IioVisitor::Visit(fdf_devicetree::Node& node,
                               const devicetree::PropertyDecoder& decoder) {
  auto iio_props = iio_parser_->Parse(node);
  if (iio_props.is_error()) {
    return iio_props.take_error();
  }

  auto iio_channels = iio_props->Get<fdf_devicetree::References>(kIioChannelReference);
  if (iio_channels) {
    auto iio_channel_names = iio_props->Get<std::vector<std::string>>(kIioChannelNames);
    if (!iio_channel_names || iio_channel_names->size() != iio_channels->size()) {
      // We need a iio names to generate bind rules.
      fdf::error("IIO reference '{}' does not have valid IIO names field.", node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    for (uint32_t index = 0; index < iio_channels->size(); index++) {
      auto& reference = (*iio_channels)[index];
      if (is_match(reference.reference_node())) {
        auto result = ParseReferenceChild(node, reference.reference_node(),
                                          reference.property_cells(), (*iio_channel_names)[index]);
        if (result.is_error()) {
          return result.take_error();
        }
      }
    }
  }
  return zx::ok();
}

}  // namespace iio_dt
