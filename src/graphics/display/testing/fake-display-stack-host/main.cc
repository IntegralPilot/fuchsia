// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <chrono>
#include <thread>

#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display-device-config.h"
#include "src/graphics/display/lib/fake-display-stack/fake-display-stack.h"
#include "src/graphics/display/lib/fake-display-stack/sysmem-service-forwarder.h"
#include "src/graphics/display/testing/fake-display-stack-host/fake_display_stack_host_config.h"

namespace {

fake_display::FakeDisplayDeviceConfig GetFakeDisplayDeviceConfigFromComponentConfig(
    const fake_display_stack_host_config::Config& component_config) {
  return fake_display::FakeDisplayDeviceConfig{
      .display_mode = display::Mode({
          .active_width = static_cast<int32_t>(component_config.active_width_px()),
          .active_height = static_cast<int32_t>(component_config.active_height_px()),
          .refresh_rate_millihertz =
              static_cast<int32_t>(component_config.refresh_rate_millihertz()),
      }),
      .engine_info = display::EngineInfo({
          .max_layer_count = static_cast<int32_t>(component_config.max_layer_count()),
          .max_connected_display_count = 1,
          .is_capture_supported = true,
      }),
      .periodic_vsync = true,
  };
}

}  // namespace

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "Starting fake fuchsia.hardware.display.Service service.";

  const fake_display_stack_host_config::Config component_config =
      fake_display_stack_host_config::Config::TakeFromStartupHandle();
  const fake_display::FakeDisplayDeviceConfig fake_display_device_config =
      GetFakeDisplayDeviceConfigFromComponentConfig(component_config);

  zx::result<std::unique_ptr<fake_display::SysmemServiceForwarder>>
      sysmem_service_forwarder_result = fake_display::SysmemServiceForwarder::Create();
  FX_CHECK(sysmem_service_forwarder_result.is_ok());

  std::unique_ptr<fake_display::SysmemServiceForwarder> sysmem_service_forwarder =
      std::move(sysmem_service_forwarder_result).value();

  fake_display::FakeDisplayStack fake_display_stack(std::move(sysmem_service_forwarder),
                                                    fake_display_device_config);

  fake_display_stack.ServeCoordinatorToProcessOutgoingDirectory();

  while (true) {
    std::this_thread::sleep_for(std::chrono::seconds(1));
  }
  return 0;
}
