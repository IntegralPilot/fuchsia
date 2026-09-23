// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_NELSON_POST_INIT_POST_INIT_H_
#define SRC_DEVICES_BOARD_DRIVERS_NELSON_POST_INIT_POST_INIT_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/wire.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/vmo/types.h>

namespace nelson {

class PostInit : public fdf::DriverBase2 {
 public:
  PostInit() : fdf::DriverBase2("post-init") {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  zx::result<> InitBoardInfo(const fdf::Namespace& incoming);
  zx::result<> SetInspectProperties(const fdf::Namespace& incoming);

  // Identifies the panel type and stores it to `panel_type_`.
  // Must be called exactly once during driver `Start()`.
  zx::result<> IdentifyPanel(const fdf::Namespace& incoming);

  // Must be called after `IdentifyPanel()`.
  zx::result<> InitDisplay();

  // Must be called after `IdentifyPanel()`.
  zx::result<> InitTouch(const fdf::Namespace& incoming);

  // Must be called after `IdentifyPanel()`.
  zx::result<> InitBacklight();

  zx::result<> SetBoardInfo();
  zx::result<> AddSelinaCompositeNode(const fdf::Namespace& incoming);
  zx::result<> EnableSelinaOsc(const fdf::Namespace& incoming);

  // Constructs a number using the value of each GPIO as one bit. The order of elements in
  // node_names determines the bits set in the result from LSB to MSB.
  zx::result<uint32_t> ReadGpios(cpp20::span<const char* const> node_names,
                                 const fdf::Namespace& incoming);

  fidl::SyncClient<fuchsia_driver_framework::Node> parent_;
  fidl::SyncClient<fuchsia_driver_framework::NodeController> controller_;

  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> pbus_;
  fidl::SyncClient<fuchsia_driver_framework::CompositeNodeManager> composite_manager_;

  uint32_t board_build_{};
  uint32_t board_option_{};

  // Initialized exactly once in `IdentifyPanel()`.
  display::PanelType panel_type_ = display::PanelType::kUnknown;

  std::unique_ptr<inspect::ComponentInspector> component_inspector_;

  inspect::Inspector inspector_;
  inspect::Node root_;
  inspect::UintProperty board_build_property_;
  inspect::UintProperty board_option_property_;
  inspect::UintProperty panel_type_property_;
};

}  // namespace nelson

#endif  // SRC_DEVICES_BOARD_DRIVERS_NELSON_POST_INIT_POST_INIT_H_
