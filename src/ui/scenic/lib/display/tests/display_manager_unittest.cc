// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/display_manager.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>
#include <lib/async/time.h>

#include <unordered_set>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/display/tests/mock_display_coordinator.h"
#include "src/ui/scenic/lib/utils/range_inclusive.h"

namespace display::test {

namespace {

constexpr uint32_t kMaxDisplayLayersCount = 2;

class DisplayManagerMockTest : public gtest::TestLoopFixture {
 public:
  // |testing::Test|
  void SetUp() override {
    TestLoopFixture::SetUp();

    async_set_default_dispatcher(dispatcher());
    display_manager_ = std::make_unique<DisplayManager>([]() {});
  }

  // |testing::Test|
  void TearDown() override {
    display_manager_.reset();
    TestLoopFixture::TearDown();
  }

  DisplayManager* display_manager() { return display_manager_.get(); }
  Display* display() { return display_manager()->default_display(); }

 private:
  std::unique_ptr<DisplayManager> display_manager_;
};

TEST_F(DisplayManagerMockTest, DisplayVsyncCallback) {
  const display::WireDisplayId kDisplayId = {.value = 1};
  const uint32_t kDisplayWidth = 1024;
  const uint32_t kDisplayHeight = 768;
  const size_t kTotalVsync = 10;
  const size_t kAcknowledgeRate = 5;

  std::unordered_set<uint64_t> cookies_sent;
  size_t num_vsync_display_received = 0;
  size_t num_vsync_acknowledgement = 0;

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  display_manager()->BindDefaultDisplayCoordinator(dispatcher(), std::move(coordinator_client),
                                                   std::move(listener_server));

  display_manager()->SetDefaultDisplayForTests(
      std::make_shared<Display>(kDisplayId, kDisplayWidth, kDisplayHeight, kMaxDisplayLayersCount));

  MockDisplayCoordinator mock_display_coordinator(WireDisplayInfo{});
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));
  mock_display_coordinator.set_acknowledge_vsync_fn(
      [&cookies_sent, &num_vsync_acknowledgement](uint64_t cookie) {
        ASSERT_TRUE(cookies_sent.contains(cookie));
        ++num_vsync_acknowledgement;
      });

  display_manager()->default_display()->AddVsyncCallback(
      [&num_vsync_display_received](zx::time_monotonic timestamp, WireConfigStamp stamp) {
        ++num_vsync_display_received;
      });

  for (size_t vsync_id = 1; vsync_id <= kTotalVsync; vsync_id++) {
    // We only require acknowledgement for every |kAcknowledgeRate| Vsync IDs.
    uint64_t cookie = (vsync_id % kAcknowledgeRate == 0) ? vsync_id : 0;

    test_loop().AdvanceTimeByEpsilon();
    fidl::OneWayStatus result = mock_display_coordinator.listener().sync()->OnVsync(
        kDisplayId, zx::time_monotonic(test_loop().Now().get()), {1u}, {cookie});
    ASSERT_TRUE(result.ok());
    if (cookie) {
      cookies_sent.insert(cookie);
    }

    // Display coordinator should handle the incoming Vsync message.
    EXPECT_TRUE(RunLoopUntilIdle());
  }

  EXPECT_EQ(num_vsync_display_received, kTotalVsync);
  EXPECT_EQ(num_vsync_acknowledgement, kTotalVsync / kAcknowledgeRate);
}

TEST_F(DisplayManagerMockTest, OnDisplayAdded) {
  static constexpr display::WireDisplayId kDisplayId = {.value = 1};
  static constexpr int kDisplayWidth = 1024;
  static constexpr int kDisplayHeight = 768;
  static constexpr int kDisplayRefreshRateHz = 60;
  static constexpr uint32_t kMaxDisplayLayersCount = 7;

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  display_manager()->BindDefaultDisplayCoordinator(dispatcher(), std::move(coordinator_client),
                                                   std::move(listener_server));

  WireDisplayMode mode = {
      .active_area = {.width = kDisplayWidth, .height = kDisplayHeight},
      .refresh_rate_millihertz = kDisplayRefreshRateHz * 1'000,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;
  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(&mode, 1),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
      .max_layer_count = kMaxDisplayLayersCount,
  };
  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));
  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(RunLoopUntilIdle());

  const Display* default_display = display_manager()->default_display();
  ASSERT_TRUE(default_display != nullptr);
  EXPECT_EQ(default_display->width_in_px(), static_cast<uint32_t>(kDisplayWidth));
  EXPECT_EQ(default_display->height_in_px(), static_cast<uint32_t>(kDisplayHeight));
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            static_cast<uint32_t>(kDisplayRefreshRateHz * 1'000));
  EXPECT_THAT(default_display->pixel_formats(), testing::ElementsAre(pixel_format));
  EXPECT_EQ(default_display->max_layer_count(), kMaxDisplayLayersCount);
}

TEST_F(DisplayManagerMockTest, MultipleDisplayVsyncCallbacks) {
  const display::WireDisplayId kDisplayId = {.value = 1};
  const uint32_t kDisplayWidth = 1024;
  const uint32_t kDisplayHeight = 768;

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  display_manager()->BindDefaultDisplayCoordinator(dispatcher(), std::move(coordinator_client),
                                                   std::move(listener_server));

  display_manager()->SetDefaultDisplayForTests(
      std::make_shared<Display>(kDisplayId, kDisplayWidth, kDisplayHeight, kMaxDisplayLayersCount));

  MockDisplayCoordinator mock_display_coordinator(WireDisplayInfo{});
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  size_t callback1_calls = 0;
  size_t callback2_calls = 0;

  auto id1 = display_manager()->default_display()->AddVsyncCallback(
      [&](auto, auto) { callback1_calls++; });
  auto id2 = display_manager()->default_display()->AddVsyncCallback(
      [&](auto, auto) { callback2_calls++; });

  // Both callbacks should be called.

  fidl::OneWayStatus result = mock_display_coordinator.listener().sync()->OnVsync(
      kDisplayId, zx::time_monotonic(0), {1u}, {0});
  ASSERT_TRUE(result.ok());
  RunLoopUntilIdle();
  EXPECT_EQ(callback1_calls, 1u);
  EXPECT_EQ(callback2_calls, 1u);

  // Remove the first callback.
  display_manager()->default_display()->RemoveVsyncCallback(id1);

  // Only the second callback should be called.
  result = mock_display_coordinator.listener().sync()->OnVsync(kDisplayId, zx::time_monotonic(0),
                                                               {1u}, {0});
  ASSERT_TRUE(result.ok());
  RunLoopUntilIdle();
  EXPECT_EQ(callback1_calls, 1u);
  EXPECT_EQ(callback2_calls, 2u);

  // Remove the second callback.
  display_manager()->default_display()->RemoveVsyncCallback(id2);

  // No callbacks should be called.
  result = mock_display_coordinator.listener().sync()->OnVsync(kDisplayId, zx::time_monotonic(0),
                                                               {1u}, {0});
  ASSERT_TRUE(result.ok());
  RunLoopUntilIdle();
  EXPECT_EQ(callback1_calls, 1u);
  EXPECT_EQ(callback2_calls, 2u);
}

TEST_F(DisplayManagerMockTest, SelectPreferredMode) {
  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kPreferredMode = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kNonPreferredMode = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };
  std::vector<WireDisplayMode> modes = {
      kPreferredMode,
      kNonPreferredMode,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();
  display_manager()->BindDefaultDisplayCoordinator(dispatcher(), std::move(coordinator_client),
                                                   std::move(listener_server));

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));
  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(RunLoopUntilIdle());

  const Display* default_display = display_manager()->default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kPreferredMode.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kPreferredMode.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kPreferredMode.refresh_rate_millihertz);
}

TEST(DisplayManager, ICanHazDisplayMode) {
  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kPreferredMode = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kNonPreferredButSelectedMode = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };

  std::vector<WireDisplayMode> modes = {
      kPreferredMode,
      kNonPreferredButSelectedMode,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;

  async::TestLoop loop;
  async_set_default_dispatcher(loop.dispatcher());

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();
  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  DisplayManager display_manager(/*i_can_haz_display_id=*/std::nullopt,
                                 /*display_mode_index_override=*/std::make_optional(1),
                                 DisplayModeConstraints{}, inspect::Node(),
                                 /*display_available_cb=*/[]() {});
  display_manager.BindDefaultDisplayCoordinator(loop.dispatcher(), std::move(coordinator_client),
                                                std::move(listener_server));

  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(loop.RunUntilIdle());

  const Display* default_display = display_manager.default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kNonPreferredButSelectedMode.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kNonPreferredButSelectedMode.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kNonPreferredButSelectedMode.refresh_rate_millihertz);
}

TEST(DisplayManager, DisplayModeConstraintsHorizontalResolution) {
  static const DisplayModeConstraints kDisplayModeConstraints = {
      .width_px_range = utils::RangeInclusive(700, 900),
  };

  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kModeNotSatisfyingConstraints = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kModeSatisfyingConstraints = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };
  std::vector<WireDisplayMode> modes = {
      kModeNotSatisfyingConstraints,
      kModeSatisfyingConstraints,
  };
  auto pixel_format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8;

  async::TestLoop loop;
  async_set_default_dispatcher(loop.dispatcher());

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  DisplayManager display_manager(/*i_can_haz_display_id=*/std::nullopt,
                                 /*display_mode_index_override=*/std::nullopt,
                                 kDisplayModeConstraints, inspect::Node(),
                                 /*display_available_cb=*/[]() {});
  display_manager.BindDefaultDisplayCoordinator(loop.dispatcher(), std::move(coordinator_client),
                                                std::move(listener_server));

  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(loop.RunUntilIdle());

  const Display* default_display = display_manager.default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kModeSatisfyingConstraints.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kModeSatisfyingConstraints.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kModeSatisfyingConstraints.refresh_rate_millihertz);
}

TEST(DisplayManager, DisplayModeConstraintsVerticalResolution) {
  static const DisplayModeConstraints kDisplayModeConstraints = {
      .height_px_range = utils::RangeInclusive(500, 700),
  };

  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kModeNotSatisfyingConstraints = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kModeSatisfyingConstraints = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };
  std::vector<WireDisplayMode> modes = {
      kModeNotSatisfyingConstraints,
      kModeSatisfyingConstraints,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;

  async::TestLoop loop;
  async_set_default_dispatcher(loop.dispatcher());

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  DisplayManager display_manager(/*i_can_haz_display_id=*/std::nullopt,
                                 /*display_mode_index_override=*/std::nullopt,
                                 kDisplayModeConstraints, inspect::Node(),
                                 /*display_available_cb=*/[]() {});
  display_manager.BindDefaultDisplayCoordinator(loop.dispatcher(), std::move(coordinator_client),
                                                std::move(listener_server));

  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(loop.RunUntilIdle());

  const Display* default_display = display_manager.default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kModeSatisfyingConstraints.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kModeSatisfyingConstraints.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kModeSatisfyingConstraints.refresh_rate_millihertz);
}

TEST(DisplayManager, DisplayModeConstraintsRefreshRateLimit) {
  static const DisplayModeConstraints kDisplayModeConstraints = {
      .refresh_rate_millihertz_range = utils::RangeInclusive(20'000, 50'000),
  };

  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kModeNotSatisfyingConstraints = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kModeSatisfyingConstraints = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };
  std::vector<WireDisplayMode> modes = {
      kModeNotSatisfyingConstraints,
      kModeSatisfyingConstraints,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;

  async::TestLoop loop;
  async_set_default_dispatcher(loop.dispatcher());

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  DisplayManager display_manager(/*i_can_haz_display_id=*/std::nullopt,
                                 /*display_mode_index_override=*/std::nullopt,
                                 kDisplayModeConstraints, inspect::Node(),
                                 /*display_available_cb=*/[]() {});
  display_manager.BindDefaultDisplayCoordinator(loop.dispatcher(), std::move(coordinator_client),
                                                std::move(listener_server));

  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(loop.RunUntilIdle());

  const Display* default_display = display_manager.default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kModeSatisfyingConstraints.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kModeSatisfyingConstraints.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kModeSatisfyingConstraints.refresh_rate_millihertz);
}

TEST(DisplayManager, DisplayModeConstraintsOverriddenByModeIndex) {
  static const DisplayModeConstraints kDisplayModeConstraints = {
      .width_px_range = utils::RangeInclusive(700, 900),
  };

  static const display::WireDisplayId kDisplayId = {.value = 1};
  static const WireDisplayMode kModeNotSatisfyingConstraints = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60'000,
  };
  static const WireDisplayMode kModeSatisfyingConstraints = {
      .active_area = {.width = 800, .height = 600},
      .refresh_rate_millihertz = 30'000,
  };
  static const WireDisplayMode kModeOverridden = {
      .active_area = {.width = 1280, .height = 960},
      .refresh_rate_millihertz = 30'000,
  };
  std::vector<WireDisplayMode> modes = {
      kModeNotSatisfyingConstraints,
      kModeSatisfyingConstraints,
      kModeOverridden,
  };
  auto pixel_format = fuchsia_images2::PixelFormat::kR8G8B8A8;

  async::TestLoop loop;
  async_set_default_dispatcher(loop.dispatcher());

  const WireDisplayInfo kDisplayInfo = {
      .id = kDisplayId,
      .modes = fidl::VectorView<WireDisplayMode>::FromExternal(modes),
      .pixel_format =
          fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(&pixel_format, 1),
      .manufacturer_name = "manufacturer",
      .monitor_name = "model",
      .monitor_serial = "0001",
      .horizontal_size_mm = 120,
      .vertical_size_mm = 100,
      .using_fallback_size = false,
  };

  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [listener_client, listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  MockDisplayCoordinator mock_display_coordinator(kDisplayInfo);
  mock_display_coordinator.Bind(std::move(coordinator_server), std::move(listener_client));

  DisplayManager display_manager(/*i_can_haz_display_id=*/std::nullopt,
                                 /*display_mode_index_override=*/std::make_optional(2),
                                 kDisplayModeConstraints, inspect::Node(),
                                 /*display_available_cb=*/[]() {});
  display_manager.BindDefaultDisplayCoordinator(loop.dispatcher(), std::move(coordinator_client),
                                                std::move(listener_server));

  mock_display_coordinator.SendOnDisplayChangedRequest();

  EXPECT_TRUE(loop.RunUntilIdle());

  const Display* default_display = display_manager.default_display();
  ASSERT_TRUE(default_display != nullptr);

  EXPECT_EQ(default_display->width_in_px(), kModeOverridden.active_area.width);
  EXPECT_EQ(default_display->height_in_px(), kModeOverridden.active_area.height);
  EXPECT_EQ(default_display->maximum_refresh_rate_in_millihertz(),
            kModeOverridden.refresh_rate_millihertz);
}

}  // namespace

}  // namespace display::test
