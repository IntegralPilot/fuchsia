// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/sync/completion.h>

#include <optional>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/mock-fidl-display-engine-listener.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class DisplayEngineEventsFidlTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto [fidl_client, fidl_server] =
        fdf::Endpoints<fuchsia_hardware_display_engine::EngineListener>::Create();
    fidl_adapter_.SetListener(std::move(fidl_client));

    libsync::Completion mock_constructed;
    ASSERT_OK(async::PostTask(
        dispatcher_->async_dispatcher(),
        [this, &mock_constructed, fidl_server = std::move(fidl_server)]() mutable {
          mock_.emplace();
          fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener> fidl_handler =
              mock_->CreateHandler(*(dispatcher_->get()));
          fidl_handler(std::move(fidl_server));
          mock_constructed.Signal();
        }));
    mock_constructed.Wait();
  }

  void TearDown() override {
    fidl_adapter_.SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener>());
    driver_runtime_.ShutdownBackgroundDispatcher(dispatcher_->get(), [&] {
      mock_->CheckAllCallsReplayed();
      mock_.reset();
    });
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher dispatcher_{driver_runtime_.StartBackgroundDispatcher()};

  // Must be created and destroyed on `dispatcher_`.
  std::optional<display::testing::MockFidlDisplayEngineListener> mock_;

  DisplayEngineEventsFidl fidl_adapter_;
  DisplayEngineEventsInterface& interface_{fidl_adapter_};

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

TEST_F(DisplayEngineEventsFidlTest, OnDisplayAddedWithPreferredModes) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeAndId kPreferredModes[] = {
      display::ModeAndId({
          .id = display::ModeId(1),
          .mode = display::Mode({
              .active_width = 640,
              .active_height = 480,
              .refresh_rate_millihertz = 60'000,
          }),
      }),
      display::ModeAndId({
          .id = display::ModeId(2),
          .mode = display::Mode({
              .active_width = 640,
              .active_height = 480,
              .refresh_rate_millihertz = 30'000,
          }),
      }),
      display::ModeAndId({
          .id = display::ModeId(3),
          .mode = display::Mode({
              .active_width = 800,
              .active_height = 600,
              .refresh_rate_millihertz = 30'000,
          }),
      }),
  };
  static constexpr std::span<const display::PixelFormat> kEmptyPixelFormats;

  mock_->ExpectOnDisplayAdded(
      [&](fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
          fdf::Arena& arena,
          fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>::
              OnDisplayAddedCompleter::Sync& completer) {
        fuchsia_hardware_display_engine::wire::RawDisplayInfo& display_info = request->display_info;

        EXPECT_EQ(42u, display_info.display_id.value);

        ASSERT_EQ(3u, display_info.preferred_modes.size());

        EXPECT_EQ(640u, display_info.preferred_modes[0].active_area.width);
        EXPECT_EQ(480u, display_info.preferred_modes[0].active_area.height);
        EXPECT_EQ(60'000u, display_info.preferred_modes[0].refresh_rate_millihertz);

        EXPECT_EQ(640u, display_info.preferred_modes[1].active_area.width);
        EXPECT_EQ(480u, display_info.preferred_modes[1].active_area.height);
        EXPECT_EQ(30'000u, display_info.preferred_modes[1].refresh_rate_millihertz);

        EXPECT_EQ(800u, display_info.preferred_modes[2].active_area.width);
        EXPECT_EQ(600u, display_info.preferred_modes[2].active_area.height);
        EXPECT_EQ(30'000u, display_info.preferred_modes[2].refresh_rate_millihertz);

        EXPECT_THAT(display_info.edid_bytes, ::testing::IsEmpty());
        EXPECT_THAT(display_info.pixel_formats, ::testing::IsEmpty());
      });
  interface_.OnDisplayAdded(kDisplayId, kPreferredModes, kEmptyPixelFormats);
}

// Array with `Size` elements {`starting_value`, `starting_value` + 1, ...}.
template <int Size>
constexpr std::array<uint8_t, Size> CreateArithmeticProgression(size_t starting_value) {
  std::array<uint8_t, Size> result = {};
  for (size_t i = 0; i < Size; ++i) {
    result[i] = static_cast<uint8_t>(starting_value + i);
  }
  return result;
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayAddedWithPixelFormats) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr std::span<const display::ModeAndId> kEmptyPreferredModes;
  static constexpr display::PixelFormat kPixelFormats[] = {
      display::PixelFormat::kB8G8R8A8, display::PixelFormat::kR8G8B8A8,
      display::PixelFormat::kR8G8B8, display::PixelFormat::kB8G8R8};

  static constexpr fuchsia_images2::wire::PixelFormat kExpectedFidlPixelFormats[] = {
      kPixelFormats[0].ToFidl(), kPixelFormats[1].ToFidl(), kPixelFormats[2].ToFidl(),
      kPixelFormats[3].ToFidl()};

  mock_->ExpectOnDisplayAdded(
      [&](fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
          fdf::Arena& arena,
          fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>::
              OnDisplayAddedCompleter::Sync& completer) {
        fuchsia_hardware_display_engine::wire::RawDisplayInfo& display_info = request->display_info;

        EXPECT_EQ(42u, display_info.display_id.value);
        EXPECT_THAT(display_info.preferred_modes, ::testing::IsEmpty());
        EXPECT_THAT(display_info.edid_bytes, ::testing::IsEmpty());
        EXPECT_THAT(display_info.pixel_formats,
                    ::testing::ElementsAreArray(kExpectedFidlPixelFormats));
      });
  interface_.OnDisplayAdded(kDisplayId, kEmptyPreferredModes, kPixelFormats);
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayAddedWithNoListener) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeAndId kDisplayModeAndId({
      .id = display::ModeId(1),
      .mode = display::Mode({
          .active_width = 640,
          .active_height = 480,
          .refresh_rate_millihertz = 60'000,
      }),
  });
  static constexpr std::span<const display::ModeAndId> kPreferredModes(&kDisplayModeAndId, 1);
  static constexpr display::PixelFormat kPixelFormat = display::PixelFormat::kB8G8R8A8;
  static constexpr std::span<const display::PixelFormat> kPixelFormats(&kPixelFormat, 1);

  fidl_adapter_.SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener>());
  interface_.OnDisplayAdded(kDisplayId, kPreferredModes, kPixelFormats);
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayRemoved) {
  static constexpr display::DisplayId kDisplayId(42);

  mock_->ExpectOnDisplayRemoved(
      [&](fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
          fdf::Arena& arena,
          fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>::
              OnDisplayRemovedCompleter::Sync& completer) {
        EXPECT_EQ(42u, request->display_id.value);
      });
  interface_.OnDisplayRemoved(kDisplayId);
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayRemovedWithNoListener) {
  static constexpr display::DisplayId kDisplayId(42);

  fidl_adapter_.SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener>());
  interface_.OnDisplayRemoved(kDisplayId);
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayVsync) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr zx::time_monotonic kTimestamp(42'4242);
  static constexpr display::DriverConfigStamp kConfigStamp(4242'4242);

  mock_->ExpectOnDisplayVsync(
      [&](fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
          fdf::Arena& arena,
          fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>::
              OnDisplayVsyncCompleter::Sync& completer) {
        EXPECT_EQ(42u, request->display_id.value);
        EXPECT_EQ(42'4242u, request->timestamp.get());
        EXPECT_EQ(4242'4242u, request->config_stamp.value);
      });
  interface_.OnDisplayVsync(kDisplayId, kTimestamp, kConfigStamp);
}

TEST_F(DisplayEngineEventsFidlTest, OnDisplayVsyncWithNoListener) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr zx::time_monotonic kTimestamp(42'4242);
  static constexpr display::DriverConfigStamp kConfigStamp(4242'4242);

  fidl_adapter_.SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener>());
  interface_.OnDisplayVsync(kDisplayId, kTimestamp, kConfigStamp);
}

TEST_F(DisplayEngineEventsFidlTest, OnCaptureComplete) {
  mock_->ExpectOnCaptureComplete(
      [](fdf::Arena& arena, fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>::
                                OnCaptureCompleteCompleter::Sync& completer) {});
  interface_.OnCaptureComplete();
}

TEST_F(DisplayEngineEventsFidlTest, OnCaptureCompleteWithNoListener) {
  fidl_adapter_.SetListener(fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener>());
  interface_.OnCaptureComplete();
}

}  // namespace

}  // namespace display
