// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_DRIVER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/result.h>

#include <memory>
#include <optional>

#include "src/graphics/display/drivers/goldfish-display/display-engine.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

namespace goldfish {

class DisplayDriver : public fdf::DriverBase2 {
 public:
  explicit DisplayDriver();

  DisplayDriver(const DisplayDriver&) = delete;
  DisplayDriver(DisplayDriver&&) = delete;
  DisplayDriver& operator=(const DisplayDriver&) = delete;
  DisplayDriver& operator=(DisplayDriver&&) = delete;

  ~DisplayDriver() override;

  // fdf::DriverBase:
  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;

  // Must outlive `display_engine_`.
  fdf::SynchronizedDispatcher display_event_dispatcher_;

  // Must outlive `display_engine_` and `engine_fidl_adapter_`.
  std::unique_ptr<display::DisplayEngineEventsFidl> engine_events_;

  // Must outlive `engine_fidl_adapter_`.
  std::unique_ptr<DisplayEngine> display_engine_;

  std::unique_ptr<display::DisplayEngineFidlAdapter> engine_fidl_adapter_;
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_DISPLAY_DRIVER_H_
