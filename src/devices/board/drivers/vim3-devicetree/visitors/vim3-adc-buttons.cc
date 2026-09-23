// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3-adc-buttons.h"

#include <fidl/fuchsia.buttons/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/node_add_args.h>

namespace vim3_dt {

zx::result<> Vim3AdcButtonsVisitor::DriverVisit(fdf_devicetree::Node& node,
                                                const devicetree::PropertyDecoder& decoder) {
  // Add metadata for vim3 adc buttons.
  return node.name() == "adc-buttons" ? AddAdcButtonsMetadata(node) : zx::ok();
}

zx::result<> Vim3AdcButtonsVisitor::AddAdcButtonsMetadata(fdf_devicetree::Node& node) {
  auto func_types = std::vector<fuchsia_input::ConsumerControlButton>{
      fuchsia_input::ConsumerControlButton::kFunction};
  auto func_adc_config =
      fuchsia_buttons::AdcButtonConfig().channel_idx(2).release_threshold(1'000).press_threshold(
          70);
  auto func_config = fuchsia_buttons::ButtonConfig::WithAdc(std::move(func_adc_config));
  auto func_button =
      fuchsia_buttons::Button().types(std::move(func_types)).button_config(std::move(func_config));
  std::vector<fuchsia_buttons::Button> buttons;
  buttons.emplace_back(std::move(func_button));

  // How long to wait between polling attempts.  This value should be large enough to ensure
  // polling does not overly impact system performance while being small enough to debounce and
  // ensure button presses are correctly registered.
  //
  // TODO(https//fxbug/dev/315366570): Change the driver to use an IRQ instead of polling.
  constexpr uint32_t kPollingPeriodUSec = 20'000;
  fuchsia_buttons::AdcButtonsMetadata metadata{
      {.polling_rate_usec = kPollingPeriodUSec, .buttons = std::move(buttons)}};

  fit::result persisted_metadata = fidl::Persist(metadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to perist buttons metadata {}",
               persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata adc_buttons_metadata{{
      .id = fuchsia_buttons::AdcButtonsMetadata::kSerializableName,
      .data = std::move(persisted_metadata.value()),
  }};
  node.AddMetadata(adc_buttons_metadata);

  fdf::debug("Adding vim3 adc button metadata for node '{}' ", node.name().c_str());

  return zx::ok();
}

}  // namespace vim3_dt
