// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/default.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>

#include <gtest/gtest.h>

#include "lib/fpromise/single_threaded_executor.h"
#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/drivers/coordinator/engine-driver-client-fidl.h"

namespace display_coordinator {

namespace {

inspect::Hierarchy GetInspectHierarchy(const inspect::Inspector& inspector) {
  fpromise::result<inspect::Hierarchy> hierarchy_maybe =
      fpromise::run_single_threaded(inspect::ReadFromInspector(inspector));
  EXPECT_TRUE(hierarchy_maybe.is_ok());
  return hierarchy_maybe.take_value();
}

class InspectTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto [engine_client_end, engine_server_end] =
        fdf::Endpoints<fuchsia_hardware_display_engine::Engine>::Create();
    std::unique_ptr<EngineDriverClient> engine_driver_client =
        std::make_unique<EngineDriverClientFidl>(std::move(engine_client_end));

    auto [coordinator_client_end, coordinator_server_end] =
        fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();

    controller_.emplace(std::move(engine_driver_client), driver_dispatcher_->borrow(),
                        inspect::Inspector{}, /*fallback_horizontal_size_mm=*/0,
                        /*fallback_vertical_size_mm=*/0);
  }

  void TearDown() override {
    driver_runtime_.ShutdownAllDispatchers(/*dut_initial_dispatcher=*/nullptr);
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;

  fdf::UnownedSynchronizedDispatcher driver_dispatcher_ = driver_runtime_.GetForegroundDispatcher();

  std::optional<Controller> controller_;
};

TEST_F(InspectTest, SubmitConfigHierarchy) {
  inspect::Hierarchy hierarchy = GetInspectHierarchy(controller_->inspector());
  const inspect::Hierarchy* display = hierarchy.GetByPath({"display"});
  ASSERT_NE(display, nullptr);
  const inspect::NodeValue& display_node = display->node();

  const inspect::UintPropertyValue* last_valid_apply_config_timestamp_ns =
      display_node.get_property<inspect::UintPropertyValue>("last_valid_apply_config_timestamp_ns");
  ASSERT_NE(last_valid_apply_config_timestamp_ns, nullptr);
  const inspect::UintPropertyValue* last_valid_apply_config_interval_ns =
      display_node.get_property<inspect::UintPropertyValue>("last_valid_apply_config_interval_ns");
  ASSERT_NE(last_valid_apply_config_interval_ns, nullptr);
  const inspect::UintPropertyValue* last_valid_apply_config_stamp =
      display_node.get_property<inspect::UintPropertyValue>("last_valid_apply_config_stamp");
  ASSERT_NE(last_valid_apply_config_stamp, nullptr);
  ASSERT_NE(nullptr, display_node.get_property<inspect::UintPropertyValue>(
                         "last_valid_apply_config_timestamp_mono_ns"));
  ASSERT_NE(nullptr, display_node.get_property<inspect::UintPropertyValue>(
                         "last_valid_apply_config_timestamp_boot_ns"));
  ASSERT_NE(nullptr, display_node.get_property<inspect::UintPropertyValue>(
                         "last_valid_apply_config_duration_mono_ns"));
  ASSERT_NE(nullptr, display_node.get_property<inspect::UintPropertyValue>(
                         "last_valid_apply_config_duration_boot_ns"));
}

TEST_F(InspectTest, VsyncMonitorHierarchy) {
  inspect::Hierarchy hierarchy = GetInspectHierarchy(controller_->inspector());
  const inspect::Hierarchy* vsync_monitor = hierarchy.GetByPath({"display", "vsync_monitor"});
  ASSERT_NE(vsync_monitor, nullptr);
  const inspect::NodeValue& vsync_monitor_node = vsync_monitor->node();

  // Each in its own scope, so we don't assert on a wrong property by accident.
  {
    const inspect::UintPropertyValue* last_vsync_timestamp_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_timestamp_ns");
    ASSERT_NE(last_vsync_timestamp_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_mono_timestamp_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_timestamp_mono_ns");
    ASSERT_NE(last_vsync_mono_timestamp_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_timestamp_approximate_boot_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>(
            "last_vsync_timestamp_approximate_boot_ns");
    ASSERT_NE(last_vsync_timestamp_approximate_boot_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_interval_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_interval_ns");
    ASSERT_NE(last_vsync_interval_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_interval_mono_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_interval_mono_ns");
    ASSERT_NE(last_vsync_interval_mono_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_interval_boot_ns =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_interval_boot_ns");
    ASSERT_NE(last_vsync_interval_boot_ns, nullptr);
  }
  {
    const inspect::UintPropertyValue* last_vsync_config_stamp =
        vsync_monitor_node.get_property<inspect::UintPropertyValue>("last_vsync_config_stamp");
    ASSERT_NE(last_vsync_config_stamp, nullptr);
  }

  const inspect::UintPropertyValue* vsync_stalls =
      vsync_monitor_node.get_property<inspect::UintPropertyValue>("vsync_stalls");
  ASSERT_NE(vsync_stalls, nullptr);

  ASSERT_NE(nullptr, vsync_monitor_node.get_property<inspect::UintPropertyValue>("vsync_count"));
  ASSERT_NE(nullptr, vsync_monitor_node.get_property<inspect::UintPropertyValue>(
                         "expected_vsync_interval_ns"));
  ASSERT_NE(nullptr,
            vsync_monitor_node.get_property<inspect::IntPropertyValue>("last_vsync_jitter_ns"));
  ASSERT_NE(nullptr, vsync_monitor_node.get_property<inspect::UintPropertyValue>(
                         "time_since_last_vsync_sampled_ns"));
  ASSERT_NE(nullptr, vsync_monitor_node.get_property<inspect::UintPropertyValue>(
                         "time_since_last_vsync_ns"));
  ASSERT_NE(nullptr,
            vsync_monitor_node.get_property<inspect::UintArrayValue>("vsync_frequency_hz"));
  ASSERT_NE(nullptr, vsync_monitor_node.get_property<inspect::IntArrayValue>("vsync_jitter_us"));
}

}  // namespace

}  // namespace display_coordinator
