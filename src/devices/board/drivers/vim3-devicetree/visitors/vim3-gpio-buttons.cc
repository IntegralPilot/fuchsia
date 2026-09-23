// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3-gpio-buttons.h"

#include <fidl/fuchsia.buttons/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>

namespace vim3_dt {

zx::result<> Vim3GpioButtonsVisitor::DriverVisit(fdf_devicetree::Node& node,
                                                 const devicetree::PropertyDecoder& decoder) {
  static const std::vector<fuchsia_buttons::GpioButtonConfig> kButtons = {{{
      .type = fuchsia_buttons::GpioButtonType::WithDirect({}),
      .gpio_a_index = 0,
      .id = fuchsia_buttons::GpioButtonId::kPower,
  }}};

  static const std::vector<fuchsia_buttons::GpioConfig> kGpios = {
      {{.type = fuchsia_buttons::GpioType::WithInterrupt({}),
        .flags = fuchsia_buttons::GpioFlag::kInverted}},
  };

  static const fuchsia_buttons::GpioButtonsMetadata kMetadata(
      {.buttons = kButtons, .gpios = kGpios});

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to persist pin metadata: {}",
               persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata metadata(
      {.id = fuchsia_buttons::GpioButtonsMetadata::kSerializableName,
       .data = std::move(persisted_metadata.value())});

  node.AddMetadata(std::move(metadata));

  return zx::ok();
}

}  // namespace vim3_dt
