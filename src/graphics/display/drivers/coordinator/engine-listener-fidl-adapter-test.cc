// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/engine-listener-fidl-adapter.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/completion.h>

#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/testing/mock-engine-listener.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {
class EngineListenerFidlAdapterTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto [client_end, server_end] =
        fdf::Endpoints<fuchsia_hardware_display_engine::EngineListener>::Create();
    events_.SetListener(std::move(client_end));

    ASSERT_OK(async::PostTask(
        dispatcher_->async_dispatcher(), [this, server_end = std::move(server_end)]() mutable {
          adapter_.emplace(&mock_engine_listener_, dispatcher_->borrow());
          fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener> handler =
              adapter_->CreateHandler();
          handler(std::move(server_end));
        }));
  }

  void TearDown() override {
    driver_runtime_.ShutdownBackgroundDispatcher(dispatcher_->get(), [this] {
      adapter_.reset();
      mock_engine_listener_.CheckAllCallsReplayed();
    });
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher dispatcher_{driver_runtime_.StartBackgroundDispatcher()};

  testing::MockEngineListener mock_engine_listener_;
  std::optional<EngineListenerFidlAdapter> adapter_;
  display::DisplayEngineEventsFidl events_;
};

TEST_F(EngineListenerFidlAdapterTest, OnDisplayAdded) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr display::PixelFormat kPixelFormats[] = {
      display::PixelFormat::kR8G8B8A8,
      display::PixelFormat::kB8G8R8A8,
  };
  static constexpr display::ModeAndId kPreferredModes[] = {
      {{
          .id = display::ModeId(1),
          .mode = display::Mode(
              {.active_width = 1024, .active_height = 768, .refresh_rate_millihertz = 60'000}),
      }},
  };

  libsync::Completion completion;
  mock_engine_listener_.ExpectOnDisplayAdded([&](std::unique_ptr<AddedDisplayInfo> info) {
    ASSERT_EQ(fdf::Dispatcher::GetCurrent()->get(), dispatcher_->get());

    EXPECT_EQ(info->display_id, kDisplayId);
    EXPECT_THAT(info->pixel_formats, ::testing::ElementsAre(display::PixelFormat::kR8G8B8A8,
                                                            display::PixelFormat::kB8G8R8A8));

    ASSERT_EQ(info->preferred_modes.size(), 1u);
    const auto& preferred_mode = info->preferred_modes[0];
    EXPECT_EQ(preferred_mode.active_area().width(), 1024);
    EXPECT_EQ(preferred_mode.active_area().height(), 768);
    EXPECT_EQ(preferred_mode.refresh_rate_millihertz(), 60'000);

    completion.Signal();
  });

  events_.OnDisplayAdded(kDisplayId, kPreferredModes, kPixelFormats);
  completion.Wait();
}

TEST_F(EngineListenerFidlAdapterTest, OnDisplayRemoved) {
  static constexpr display::DisplayId kDisplayId(1);

  libsync::Completion completion;
  mock_engine_listener_.ExpectOnDisplayRemoved([&](display::DisplayId display_id) {
    ASSERT_EQ(fdf::Dispatcher::GetCurrent()->get(), dispatcher_->get());

    EXPECT_EQ(display_id, kDisplayId);
    completion.Signal();
  });

  events_.OnDisplayRemoved(kDisplayId);
  completion.Wait();
}

TEST_F(EngineListenerFidlAdapterTest, OnDisplayVsync) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr zx::time_monotonic kTimestamp(100'200);
  static constexpr display::DriverConfigStamp kConfigStamp(123);

  libsync::Completion completion;
  mock_engine_listener_.ExpectOnDisplayVsync([&](display::DisplayId display_id,
                                                 zx::time_monotonic timestamp,
                                                 display::DriverConfigStamp driver_config_stamp) {
    ASSERT_EQ(fdf::Dispatcher::GetCurrent()->get(), dispatcher_->get());

    EXPECT_EQ(display_id, kDisplayId);
    EXPECT_EQ(timestamp, kTimestamp);
    EXPECT_EQ(driver_config_stamp, kConfigStamp);
    completion.Signal();
  });

  events_.OnDisplayVsync(kDisplayId, kTimestamp, kConfigStamp);
  completion.Wait();
}

TEST_F(EngineListenerFidlAdapterTest, OnCaptureComplete) {
  libsync::Completion completion;
  mock_engine_listener_.ExpectOnCaptureComplete([&] {
    ASSERT_EQ(fdf::Dispatcher::GetCurrent()->get(), dispatcher_->get());

    completion.Signal();
  });

  events_.OnCaptureComplete();
  completion.Wait();
}

}  // namespace

}  // namespace display_coordinator
