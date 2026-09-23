// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/coordinator_proxy.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/time.h>

#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/scenic/lib/display/tests/mock_display_coordinator.h"
#include "src/ui/scenic/lib/utils/helpers.h"

namespace display::test {

class CoordinatorProxyTest : public gtest::RealLoopFixture {
 public:
  static constexpr uint32_t kMaxDisplayLayersCount = 2;

  constexpr static std::array<WireDisplayMode, 2> kSupportedDisplayModes = {
      fuchsia_hardware_display_types::wire::Mode{.active_area = {.width = 1024, .height = 768},
                                                 .refresh_rate_millihertz = 60000,
                                                 .flags = {}},
      fuchsia_hardware_display_types::wire::Mode{.active_area = {.width = 1920, .height = 1080},
                                                 .refresh_rate_millihertz = 60000,
                                                 .flags = {}}};

  // Used to construct the default `mock_coordinator_` that is available in all tests.  It is also
  // available to tests that need to create their own mock coordinator (and `CoordinatorProxy`).
  std::pair<std::unique_ptr<testing::StrictMock<MockDisplayCoordinator>>,
            std::unique_ptr<CoordinatorProxy>>
  MakeMockDisplayCoordinatorAndCoordinatorProxy(
      CoordinatorProxy::CheckConfigHeuristics check_conifg_heuristics = {}) {
    zx::result endpoints_result = fidl::CreateEndpoints<fuchsia_hardware_display::Coordinator>();
    FX_CHECK(endpoints_result.is_ok())
        << "Failed to create FIDL endpoints for the display coordinator: "
        << endpoints_result.status_string();
    auto [coordinator_client, coordinator_server] = std::move(endpoints_result).value();

    fuchsia_hardware_display::wire::Info display_info = {
        .modes = fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(
            const_cast<WireDisplayMode*>(kSupportedDisplayModes.data()),
            kSupportedDisplayModes.size()),
        .max_layer_count = kMaxDisplayLayersCount,
    };

    auto mock_coordinator =
        std::make_unique<testing::StrictMock<MockDisplayCoordinator>>(display_info);
    // The fidl::Server requires the binding and teardown to occur on the
    // same thread where the FIDL server runs.
    libsync::Completion completion;
    async::PostTask(display_coordinator_loop_.dispatcher(),
                    [this, &mock_coordinator, &completion,
                     coordinator_server = std::move(coordinator_server)]() mutable {
                      mock_coordinator->Bind(std::move(coordinator_server), {},
                                             display_coordinator_loop_.dispatcher());
                      completion.Signal();
                    });
    completion.Wait();

    auto coordinator_proxy = std::make_unique<CoordinatorProxy>(
        std::move(coordinator_client), dispatcher(), check_conifg_heuristics);

    return std::pair{std::move(mock_coordinator), std::move(coordinator_proxy)};
  }

  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    sysmem_allocator_ = utils::CreateSysmemAllocatorClient(dispatcher(), "CoordinatorProxyTest");

    display_coordinator_loop_.StartThread("display-coordinator-loop");

    auto mock_and_proxy = MakeMockDisplayCoordinatorAndCoordinatorProxy();
    mock_coordinator_ = std::move(mock_and_proxy.first);
    coordinator_proxy_ = std::move(mock_and_proxy.second);
  }

  void TearDown() override {
    coordinator_proxy_.reset();

    // This is to make sure that the display coordinator loop has finished
    // handling all its pending tasks.
    RunLoopUntilIdle();
    libsync::Completion completion;
    async::PostTask(display_coordinator_loop_.dispatcher(), [this, &completion]() mutable {
      display_coordinator_loop_.RunUntilIdle();
      mock_coordinator_.reset();
      completion.Signal();
    });
    completion.Wait();
    display_coordinator_loop_.Quit();
    display_coordinator_loop_.JoinThreads();

    gtest::RealLoopFixture::TearDown();
  }

  // Synchronously calls `GetLatestCommittedConfigStamp()` on the display coordinator.  When we
  // receive the response from this, we know that the coordinator has also received/processed any
  // previously-sent messages.
  void PingMockDisplayCoordinator(CoordinatorProxy* proxy = nullptr) {
    auto _ =
        (proxy ? proxy : coordinator_proxy_.get())->raw().sync()->GetLatestCommittedConfigStamp();
  }

 protected:
  static constexpr fuchsia_images2::PixelFormat kPixelFormat =
      fuchsia_images2::PixelFormat::kB8G8R8A8;

  async::Loop display_coordinator_loop_{&kAsyncLoopConfigNeverAttachToThread};

  std::unique_ptr<MockDisplayCoordinator> mock_coordinator_;
  std::unique_ptr<CoordinatorProxy> coordinator_proxy_;

  // Only for use on the main thread. Establish a new connection when on the MockDisplayCoordinator
  // thread.
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
};

// Test that CreateLayer() and DestroyLayer() immediately send the corresponding FIDL calls.
TEST_F(CoordinatorProxyTest, CreateAndDestroyLayer) {
  bool create_layer_called = false;
  mock_coordinator_->set_create_layer_fn([&](WireLayerId layer_id) { create_layer_called = true; });

  std::optional<LayerId> destroyed_layer_id;
  mock_coordinator_->set_destroy_layer_fn(
      [&](fuchsia_hardware_display::wire::CoordinatorDestroyLayerRequest* request) {
        destroyed_layer_id = LayerId(request->layer_id);
      });

  // CreateLayer should be called on the mock.
  const LayerId layer_id = coordinator_proxy_->CreateLayer();
  EXPECT_TRUE(create_layer_called);
  EXPECT_NE(layer_id, kInvalidLayerId);

  // DestroyLayer should be called on the mock with the correct ID.
  coordinator_proxy_->DestroyLayer(layer_id);
  PingMockDisplayCoordinator();
  EXPECT_TRUE(destroyed_layer_id.has_value());

  EXPECT_EQ(destroyed_layer_id.value(), layer_id);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test that SetDisplayMode() does not immediately send a FIDL call, but that the call is sent
// when ApplyConfig() is called.
TEST_F(CoordinatorProxyTest, SetAndApplyDisplayMode) {
  bool set_display_mode_called = false;
  WireDisplayId captured_display_id = {};
  WireDisplayMode captured_mode = {};

  mock_coordinator_->set_display_mode_fn([&](WireDisplayId display_id, WireDisplayMode mode) {
    set_display_mode_called = true;
    captured_display_id = display_id;
    captured_mode = mode;
  });

  // SetDisplayMode should not call the mock.
  const DisplayId display_id(1);
  const DisplayMode display_mode({.active_area = types::Extent2({.width = 1920, .height = 1080}),
                                  .refresh_rate_millihertz = 60000,
                                  .mode_flags = 0});
  coordinator_proxy_->SetDisplayMode(display_id, display_mode);
  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_display_mode_called);

  // ApplyConfig should call the mock.
  const WireConfigStamp config_stamp{.value = 1};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp).status_value(), ZX_OK);
  PingMockDisplayCoordinator();
  EXPECT_TRUE(set_display_mode_called);
  EXPECT_EQ(captured_display_id.value, display_id.value());
  EXPECT_EQ(captured_mode.active_area.width, 1920u);
  EXPECT_EQ(captured_mode.active_area.height, 1080u);
  EXPECT_EQ(captured_mode.refresh_rate_millihertz, 60000u);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test that SetDisplayColorConversion() does not immediately send a FIDL call, but that the call
// is sent when ApplyConfig() is called.
TEST_F(CoordinatorProxyTest, SetAndApplyDisplayColorConversion) {
  bool set_display_color_conversion_called = false;
  WireDisplayId captured_display_id = {};
  std::array<float, 3> captured_preblend;
  std::array<float, 9> captured_coefficients;
  std::array<float, 3> captured_postblend;

  mock_coordinator_->set_display_color_conversion_fn(
      [&](WireDisplayId display_id, fidl::Array<float, 3> preblend,
          fidl::Array<float, 9> coefficients, fidl::Array<float, 3> postblend) {
        set_display_color_conversion_called = true;
        captured_display_id = display_id;

        memcpy(captured_preblend.data(), preblend.data(), sizeof(captured_preblend));
        memcpy(captured_coefficients.data(), coefficients.data(), sizeof(captured_coefficients));
        memcpy(captured_postblend.data(), postblend.data(), sizeof(captured_postblend));
      });

  // SetDisplayColorConversion should not call the mock.
  const DisplayId display_id(1);
  const std::array<float, 3> kExpectedPreblend = {0.1f, 0.2f, 0.3f};
  const std::array<float, 9> kExpectedCoefficients = {1.f, 0.f, 0.f, 0.f, 1.f, 0.f, 0.f, 0.f, 1.f};
  const std::array<float, 3> kExpectedPostblend = {0.4f, 0.5f, 0.6f};
  coordinator_proxy_->SetDisplayColorConversion(display_id, kExpectedPreblend,
                                                kExpectedCoefficients, kExpectedPostblend);
  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_display_color_conversion_called);

  // ApplyConfig should call the mock.
  const WireConfigStamp config_stamp{.value = 1};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp).status_value(), ZX_OK);
  PingMockDisplayCoordinator();
  EXPECT_TRUE(set_display_color_conversion_called);
  EXPECT_EQ(captured_display_id.value, display_id.value());
  EXPECT_EQ(captured_preblend, kExpectedPreblend);
  EXPECT_EQ(captured_coefficients, kExpectedCoefficients);
  EXPECT_EQ(captured_postblend, kExpectedPostblend);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test that SetDisplayLayers() does not immediately send a FIDL call, but that the call is sent
// when ApplyConfig() is called.
TEST_F(CoordinatorProxyTest, SetAndApplyDisplayLayers) {
  bool set_display_layers_called = false;
  WireDisplayId captured_display_id = {};
  std::vector<LayerId> captured_layers;

  mock_coordinator_->set_set_display_layers_fn(
      [&](WireDisplayId display_id, fidl::VectorView<WireLayerId> layers) {
        set_display_layers_called = true;
        captured_display_id = display_id;
        captured_layers.clear();
        captured_layers.reserve(layers.size());
        for (const auto& wire_layer : layers) {
          captured_layers.push_back(LayerId(wire_layer));
        }
      });

  // Create a couple of layers to use.
  const LayerId layer_id1 = coordinator_proxy_->CreateLayer();
  const LayerId layer_id2 = coordinator_proxy_->CreateLayer();

  // SetDisplayLayers should not call the mock.
  const DisplayId display_id(1);
  const std::vector<LayerId> kExpectedLayers = {layer_id1, layer_id2};
  coordinator_proxy_->SetDisplayLayers(display_id, std::span<const LayerId>(kExpectedLayers));
  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_display_layers_called);

  // ApplyConfig should call the mock.
  const WireConfigStamp config_stamp{.value = 1};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp).status_value(), ZX_OK);
  PingMockDisplayCoordinator();
  EXPECT_TRUE(set_display_layers_called);
  EXPECT_EQ(captured_display_id.value, display_id.value());
  EXPECT_EQ(captured_layers, kExpectedLayers);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test that SetLayerPrimaryConfig(), SetLayerPrimaryPosition(), SetLayerPrimaryAlpha(), and
// SetLayerImage() do not immediately send FIDL calls, but that the calls are sent when
// ApplyConfig() is called.
TEST_F(CoordinatorProxyTest, SetAndApplyLayerImage) {
  bool set_layer_primary_config_called = false;
  WireLayerId captured_config_layer_id = {};
  WireImageMetadata captured_image_metadata = {};
  mock_coordinator_->set_set_layer_primary_config_fn(
      [&](WireLayerId layer_id, WireImageMetadata image_metadata) {
        set_layer_primary_config_called = true;
        captured_config_layer_id = layer_id;
        captured_image_metadata = image_metadata;
      });

  bool set_layer_primary_position_called = false;
  WireLayerId captured_position_layer_id = {};
  WireCoordinateTransformation captured_transform = {};
  WireRectU captured_src_frame = {};
  WireRectU captured_dest_frame = {};
  mock_coordinator_->set_layer_primary_position_fn([&](WireLayerId layer_id,
                                                       WireCoordinateTransformation transform,
                                                       WireRectU src_frame, WireRectU dest_frame) {
    set_layer_primary_position_called = true;
    captured_position_layer_id = layer_id;
    captured_transform = transform;
    captured_src_frame = src_frame;
    captured_dest_frame = dest_frame;
  });

  bool set_layer_primary_alpha_called = false;
  WireLayerId captured_alpha_layer_id = {};
  WireAlphaMode captured_alpha_mode = {};
  float captured_alpha_val = 0.f;
  mock_coordinator_->set_set_layer_primary_alpha_fn(
      [&](WireLayerId layer_id, WireAlphaMode mode, float val) {
        set_layer_primary_alpha_called = true;
        captured_alpha_layer_id = layer_id;
        captured_alpha_mode = mode;
        captured_alpha_val = val;
      });

  bool set_layer_image_called = false;
  WireLayerId captured_image_layer_id = {};
  WireImageId captured_image_id = {};
  WireEventId captured_wait_event_id = {};
  mock_coordinator_->set_set_layer_image_fn(
      [&](WireLayerId layer_id, WireImageId image_id, WireEventId wait_event_id) {
        set_layer_image_called = true;
        captured_image_layer_id = layer_id;
        captured_image_id = image_id;
        captured_wait_event_id = wait_event_id;
      });

  // Create a layer to use.
  const LayerId layer_id = coordinator_proxy_->CreateLayer();

  // Call the SetLayer* methods. These should not call the mock yet.
  const ImageId kImageId(1);
  const types::Extent2 kImageExtent({.width = 100, .height = 100});
  const uint32_t kImageTilingType = 0;
  coordinator_proxy_->SetLayerPrimaryConfig(layer_id, kImageExtent, kImageTilingType);

  const RotateFlip kTransform = RotateFlip::kReflectY();
  const Rectangle kSrcFrame({.x = 0, .y = 0, .width = 100, .height = 100});
  const Rectangle kDestFrame({.x = 0, .y = 0, .width = 200, .height = 200});
  coordinator_proxy_->SetLayerPrimaryPosition(layer_id, kTransform, kSrcFrame, kDestFrame);

  const BlendMode kBlendMode = BlendMode::kStraightAlpha();
  const float kAlpha = 0.5f;
  coordinator_proxy_->SetLayerPrimaryAlpha(layer_id, kBlendMode, kAlpha);

  const EventId kWaitEventId(1);
  coordinator_proxy_->SetLayerImage(layer_id, kImageId, kWaitEventId);

  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_layer_primary_config_called);
  EXPECT_FALSE(set_layer_primary_position_called);
  EXPECT_FALSE(set_layer_primary_alpha_called);
  EXPECT_FALSE(set_layer_image_called);

  // ApplyConfig should call the mock functions.
  const WireConfigStamp config_stamp{.value = 1};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp).status_value(), ZX_OK);
  PingMockDisplayCoordinator();

  EXPECT_TRUE(set_layer_primary_config_called);
  EXPECT_EQ(captured_config_layer_id.value, layer_id.value());
  EXPECT_EQ(Extent2::From(captured_image_metadata.dimensions), kImageExtent);
  EXPECT_EQ(captured_image_metadata.tiling_type, kImageTilingType);

  EXPECT_TRUE(set_layer_primary_position_called);
  EXPECT_EQ(captured_position_layer_id.value, layer_id.value());
  EXPECT_EQ(RotateFlip::From(captured_transform), kTransform);
  EXPECT_EQ(Rectangle::From(captured_src_frame), kSrcFrame);
  EXPECT_EQ(Rectangle::From(captured_dest_frame), kDestFrame);

  EXPECT_TRUE(set_layer_primary_alpha_called);
  EXPECT_EQ(captured_alpha_layer_id.value, layer_id.value());
  EXPECT_EQ(BlendMode::From(captured_alpha_mode), kBlendMode);
  EXPECT_EQ(captured_alpha_val, kAlpha);

  EXPECT_TRUE(set_layer_image_called);
  EXPECT_EQ(captured_image_layer_id.value, layer_id.value());
  EXPECT_EQ(captured_image_id.value, kImageId.value());
  EXPECT_EQ(captured_wait_event_id.value, kWaitEventId.value());

  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test that SetLayerColorConfig() does not immediately send a FIDL call, but that the call is
// sent when ApplyConfig() is called.  Also test transitioning between image and color layers.
TEST_F(CoordinatorProxyTest, SetAndApplyLayerColor) {
  bool set_layer_color_config_called = false;
  WireLayerId captured_layer_id = {};
  WireColor captured_color = {};
  WireRectU captured_dest_frame = {};

  mock_coordinator_->set_set_layer_color_config_fn(
      [&](WireLayerId layer_id, WireColor color, WireRectU dest_frame) {
        set_layer_color_config_called = true;
        captured_layer_id = layer_id;
        captured_color = color;
        captured_dest_frame = dest_frame;
      });

  // Create a layer to use.
  const LayerId layer_id = coordinator_proxy_->CreateLayer();

  // Call SetLayerColorConfig. This should not call the mock yet.
  const std::array<float, 4> kColorBgra = {0.1f, 0.2f, 0.3f, 0.4f};
  const WireColor kWireColor = {
      .format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
      .bytes =
          {
              static_cast<uint8_t>(kColorBgra[0] * 255.f),  // Blue
              static_cast<uint8_t>(kColorBgra[1] * 255.f),  // Green
              static_cast<uint8_t>(kColorBgra[2] * 255.f),  // Red
              static_cast<uint8_t>(kColorBgra[3] * 255.f),  // Alpha
          },
  };

  const Rectangle kDestFrame({.x = 10, .y = 20, .width = 100, .height = 200});
  coordinator_proxy_->SetLayerColorConfig(layer_id, kWireColor, kDestFrame);

  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_layer_color_config_called);

  // ApplyConfig should call the mock function.
  const WireConfigStamp config_stamp1{.value = 1};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp1).status_value(), ZX_OK);
  PingMockDisplayCoordinator();

  EXPECT_TRUE(set_layer_color_config_called);
  EXPECT_EQ(captured_layer_id.value, layer_id.value());
  EXPECT_EQ(captured_color.format, fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  EXPECT_EQ(captured_color.bytes, kWireColor.bytes);
  EXPECT_EQ(Rectangle::From(captured_dest_frame), kDestFrame);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);

  // Transition to an image layer.
  set_layer_color_config_called = false;
  const ImageId kImageId(1);
  const EventId kWaitEventId(1);
  coordinator_proxy_->SetLayerImage(layer_id, kImageId, kWaitEventId);

  const WireConfigStamp config_stamp2{.value = 2};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp2).status_value(), ZX_OK);
  PingMockDisplayCoordinator();
  EXPECT_FALSE(set_layer_color_config_called);  // Should not be called

  // Transition back to a color layer (RGBA this time, not BGRA).
  const std::array<float, 4> kColorRgba = {0.5f, 0.6f, 0.7f, 0.8f};
  const WireColor kWireColor2 = {
      .format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
      .bytes =
          {
              static_cast<uint8_t>(kColorRgba[0] * 255.f),  // Red
              static_cast<uint8_t>(kColorRgba[1] * 255.f),  // Green
              static_cast<uint8_t>(kColorRgba[2] * 255.f),  // Blue
              static_cast<uint8_t>(kColorRgba[3] * 255.f),  // Alpha
          },
  };

  const Rectangle kDestFrame2({.x = 30, .y = 40, .width = 150, .height = 250});
  coordinator_proxy_->SetLayerColorConfig(layer_id, kWireColor2, kDestFrame2);

  const WireConfigStamp config_stamp3{.value = 3};
  EXPECT_EQ(coordinator_proxy_->ApplyConfig(config_stamp3).status_value(), ZX_OK);
  PingMockDisplayCoordinator();

  EXPECT_TRUE(set_layer_color_config_called);
  EXPECT_EQ(captured_layer_id.value, layer_id.value());
  EXPECT_EQ(captured_color.format, fuchsia_images2::wire::PixelFormat::kR8G8B8A8);
  EXPECT_EQ(captured_color.bytes, kWireColor2.bytes);
  EXPECT_EQ(Rectangle::From(captured_dest_frame), kDestFrame2);
  EXPECT_EQ(mock_coordinator_->illegal_action_count(), 0u);
}

// Test the three states of the CheckConfig cache:
// 1. Not cached, success: CheckConfig() is called, returns success, and ApplyConfig() is called.
// 2. Cached, success: CheckConfig() is not called, but ApplyConfig() is.
// 3. Not cached, failure: CheckConfig() is called, returns failure, ApplyConfig() is not called,
//    and DiscardConfig() is.
// 4. Cached, failure: No FIDL calls are made, and an error is returned.
TEST_F(CoordinatorProxyTest, ApplyConfig_CheckConfigCache) {
  // Create a couple of layers to use.
  const std::array<LayerId, 2> layer_ids = {coordinator_proxy_->CreateLayer(),
                                            coordinator_proxy_->CreateLayer()};
  const LayerId& color_layer_id = layer_ids[0];
  const LayerId& image_layer_id = layer_ids[1];

  const DisplayId kDisplayID(1);

  // Set up color layer.
  {
    const std::array<float, 4> kColorBgra = {0.1f, 0.2f, 0.3f, 0.4f};
    const WireColor kWireColor = {
        .format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
        .bytes =
            {
                static_cast<uint8_t>(kColorBgra[0] * 255.f),  // Blue
                static_cast<uint8_t>(kColorBgra[1] * 255.f),  // Green
                static_cast<uint8_t>(kColorBgra[2] * 255.f),  // Red
                static_cast<uint8_t>(kColorBgra[3] * 255.f),  // Alpha
            },
    };

    const Rectangle kDestFrame({.x = 10, .y = 20, .width = 100, .height = 200});
    coordinator_proxy_->SetLayerColorConfig(color_layer_id, kWireColor, kDestFrame);
  }

  // Set up image layer.
  {
    const types::Extent2 kImageExtent({.width = 100, .height = 100});
    const uint32_t kImageTilingType = 0;
    coordinator_proxy_->SetLayerPrimaryConfig(image_layer_id, kImageExtent, kImageTilingType);

    const RotateFlip kTransform = RotateFlip::kReflectY();
    const Rectangle kSrcFrame({.x = 0, .y = 0, .width = 100, .height = 100});
    const Rectangle kDestFrame({.x = 0, .y = 0, .width = 200, .height = 200});
    coordinator_proxy_->SetLayerPrimaryPosition(image_layer_id, kTransform, kSrcFrame, kDestFrame);

    const BlendMode kBlendMode = BlendMode::kStraightAlpha();
    const float kAlpha = 0.5f;
    coordinator_proxy_->SetLayerPrimaryAlpha(image_layer_id, kBlendMode, kAlpha);

    const ImageId kImageId(1);
    const EventId kWaitEventId(1);
    coordinator_proxy_->SetLayerImage(image_layer_id, kImageId, kWaitEventId);
  }

  // Verify count of API calls made to the proxy.  None have been over FIDL yet.
  EXPECT_EQ(coordinator_proxy_->api_calls_received(),
            // 2 CreateLayer +
            // 1 SetLayerColorConfig +
            // 3 SetLayerPrimaryConfig/Position/Alpha +
            // 1 SetLayerImage
            7U);
  EXPECT_EQ(coordinator_proxy_->api_calls_sent(),
            // only layer creation occurs immediately
            2U);

  // The mock coordinator doesn't care whether the stamp is strictly increasing.
  const WireConfigStamp kConfigStamp{.value = 1};

  // We'll pretend that only 1 layer is allowed, so either the color layer or image layer alone is
  // OK, but both together means that `CheckConfig()` fails.

  // The first two single layer configs will succeed.
  mock_coordinator_->set_check_config_fn(
      [](WireConfigResult* out) { *out = WireConfigResult::kOk; });

  coordinator_proxy_->SetDisplayLayers(kDisplayID, {&color_layer_id, 1});
  auto result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_ok());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 1U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 1U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 0U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 1U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 1U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 0U);

  coordinator_proxy_->SetDisplayLayers(kDisplayID, {&image_layer_id, 1});
  result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_ok());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 2U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 2U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 0U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 2U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 2U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 0U);

  // Verify count of API calls made to the proxy.  All have been sent over FIDL.
  EXPECT_EQ(coordinator_proxy_->api_calls_received(),
            // 7 (previous value) +
            // 2 SetDisplayLayers
            9U);
  EXPECT_EQ(coordinator_proxy_->api_calls_sent(),
            // TODO(https://fxbug.dev/449807074): +1 extra call to `SetLayerImage`.
            9U + 1U);

  // The two-layer config will fail.
  mock_coordinator_->set_check_config_fn(
      [](WireConfigResult* out) { *out = WireConfigResult::kInvalidConfig; });

  coordinator_proxy_->SetDisplayLayers(kDisplayID, layer_ids);
  result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 3U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 2U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 0U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 3U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 3U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 1U);

  // Verify count of API calls made to the proxy.  All have been sent over FIDL.
  EXPECT_EQ(coordinator_proxy_->api_calls_received(),
            // 9 (previous value) +
            // 1 SetDisplayLayers
            10U);
  EXPECT_EQ(coordinator_proxy_->api_calls_sent(),
            // TODO(https://fxbug.dev/449807074): +1 previous +1 new extra call to `SetLayerImage`.
            10U + 2U);

  // Go through the same 3 configs.  We should get the same results as before, but without needing
  // to call `CheckConfig()`.
  mock_coordinator_->set_check_config_fn([](WireConfigResult* out) { ASSERT_TRUE(false); });

  coordinator_proxy_->SetDisplayLayers(kDisplayID, {&color_layer_id, 1});
  result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_ok());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 4U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 3U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 1U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 3U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 3U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 1U);

  coordinator_proxy_->SetDisplayLayers(kDisplayID, {&image_layer_id, 1});
  result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_ok());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 5U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 4U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 2U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 3U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 3U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 1U);

  coordinator_proxy_->SetDisplayLayers(kDisplayID, layer_ids);
  result = coordinator_proxy_->ApplyConfig(kConfigStamp);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_received(), 6U);
  EXPECT_EQ(coordinator_proxy_->apply_config_calls_sent(), 4U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_skipped(), 3U);
  EXPECT_EQ(coordinator_proxy_->check_config_calls_sent(), 3U);
  PingMockDisplayCoordinator();
  EXPECT_EQ(mock_coordinator_->check_config_count(), 3U);
  EXPECT_EQ(mock_coordinator_->discard_config_count(), 1U);

  // Verify count of API calls made to the proxy.  All have been sent over FIDL.
  EXPECT_EQ(coordinator_proxy_->api_calls_received(),
            // 10 (previous value) +
            // 3 SetDisplayLayers
            13U);
  EXPECT_EQ(coordinator_proxy_->api_calls_sent(),
            // 10 (previous value) +
            // 2 SetDisplayLayers
            // NOTE: the third SetDisplayLayers is not sent because the proxy was able to determine
            //       that CheckConfig would fail without calling SetDisplayLayers, and then needing
            //       to subsequently call DiscardConfig to clean up afterward.
            // TODO(https://fxbug.dev/449807074): +2 previous +2 new extra call to `SetLayerImage`.
            12U + 4U);
}

// Helper for CheckConfigHeuristics test.  Sets up the display, and a simple config with one
// non-fullscreen layer, which triggers the "only fullscreen layers allowed" heuristic.
static void CheckConfigHeuristicsTestSetup(CoordinatorProxy& proxy) {
  const DisplayId display_id(1);
  const DisplayMode display_mode({.active_area = types::Extent2({.width = 1920, .height = 1080}),
                                  .refresh_rate_millihertz = 60000,
                                  .mode_flags = 0});
  proxy.SetDisplayMode(display_id, display_mode);

  const LayerId layer_id = proxy.CreateLayer();
  const ImageId kImageId(1);
  const types::Extent2 kImageExtent({.width = 100, .height = 100});
  const uint32_t kImageTilingType = 0;
  proxy.SetLayerPrimaryConfig(layer_id, kImageExtent, kImageTilingType);

  const RotateFlip kTransform = RotateFlip::kReflectY();
  const Rectangle kSrcFrame({.x = 0, .y = 0, .width = 100, .height = 100});
  const Rectangle kDestFrame({.x = 0, .y = 0, .width = 200, .height = 200});
  proxy.SetLayerPrimaryPosition(layer_id, kTransform, kSrcFrame, kDestFrame);

  const BlendMode kBlendMode = BlendMode::kStraightAlpha();
  const float kAlpha = 0.5f;
  proxy.SetLayerPrimaryAlpha(layer_id, kBlendMode, kAlpha);

  const EventId kWaitEventId(1);
  proxy.SetLayerImage(layer_id, kImageId, kWaitEventId);

  proxy.SetDisplayLayers(display_id, std::vector{layer_id});
}

// Test that enabling "check config heuristics" gives the expected results.  In particular, they
// should cause `ApplyConfig()` to fail without checking the `check_config_cache_`.
TEST_F(CoordinatorProxyTest, CheckConfigHeuristics) {
  const WireConfigStamp config_stamp{.value = 1};

  // Heuristics disabled, so `ApplyConfig()` succeeds.
  {
    auto [mock, proxy] =
        MakeMockDisplayCoordinatorAndCoordinatorProxy({.enable_heuristics = false});
    CheckConfigHeuristicsTestSetup(*proxy);
    EXPECT_EQ(proxy->ApplyConfig(config_stamp).status_value(), ZX_OK);

    // Drain all calls before destroying mock/proxy.
    PingMockDisplayCoordinator(proxy.get());
  }

  // Heuristics enabled, so `ApplyConfig()` fails.
  {
    auto [mock, proxy] = MakeMockDisplayCoordinatorAndCoordinatorProxy({.enable_heuristics = true});
    CheckConfigHeuristicsTestSetup(*proxy);
    EXPECT_EQ(proxy->ApplyConfig(config_stamp).status_value(), ZX_ERR_BAD_STATE);

    // Drain all calls before destroying mock/proxy.
    PingMockDisplayCoordinator(proxy.get());
  }
}

}  // namespace display::test
