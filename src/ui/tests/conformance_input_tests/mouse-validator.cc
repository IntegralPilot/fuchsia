// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.conformance/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>

#include <cstdint>

#include <src/ui/testing/util/zxtest_helpers.h>
#include <src/ui/tests/conformance_input_tests/conformance-test-base.h>
#include <zxtest/zxtest.h>

namespace ui_conformance_testing {

namespace futi = fuchsia_ui_test_input;
namespace futc = fuchsia_ui_test_conformance;

const std::string PUPPET_UNDER_TEST_FACTORY_SERVICE = "/svc/puppet-under-test-factory-service";

// Two physical pixel coordinates are considered equivalent if their distance is less than 1 unit.
constexpr double kPixelEpsilon = 0.5f;

// Epsilon for floating error.
constexpr double kEpsilon = 0.0001f;

class MouseListener : public fidl::Server<futi::MouseInputListener> {
 public:
  // |fuchsia_ui_test_input::MouseInputListener|
  void ReportMouseInput(ReportMouseInputRequest& request,
                        ReportMouseInputCompleter::Sync& completer) override {
    events_received_.push_back(std::move(request));
  }

  fidl::ClientEnd<futi::MouseInputListener> ServeAndGetClientEnd(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<futi::MouseInputListener>::Create();
    binding_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  const std::vector<futi::MouseInputListenerReportMouseInputRequest>& events_received() const {
    return events_received_;
  }

  void clear_events() { events_received_.clear(); }

 private:
  fidl::ServerBindingGroup<futi::MouseInputListener> binding_;
  std::vector<futi::MouseInputListenerReportMouseInputRequest> events_received_;
};

// Holds resources associated with a single puppet instance.
struct MousePuppet {
  fidl::SyncClient<futc::Puppet> client;
  MouseListener mouse_listener;
};

using device_pixel_ratio = float;

class MouseConformanceTest : public ui_conformance_test_base::ConformanceTest,
                             public zxtest::WithParamInterface<device_pixel_ratio> {
 public:
  ~MouseConformanceTest() override = default;

  float DevicePixelRatio() const override { return GetParam(); }

  void SetUp() override {
    ui_conformance_test_base::ConformanceTest::SetUp();

    // Register fake mouse.
    {
      FX_LOGS(INFO) << "Connecting to input registry";
      auto input_registry = ConnectSyncIntoRealm<futi::Registry>();

      FX_LOGS(INFO) << "Registering fake mouse";
      auto [client_end, server_end] = fidl::Endpoints<futi::Mouse>::Create();
      fake_mouse_ = fidl::SyncClient(std::move(client_end));

      futi::RegistryRegisterMouseAndGetDeviceInfoRequest request;
      request.device(std::move(server_end));

      auto res = input_registry->RegisterMouseAndGetDeviceInfo(std::move(request));
      ZX_ASSERT_OK(res);

      fake_mouse_device_id_ = res->device_id().value();
    }

    // Get display dimensions.
    {
      FX_LOGS(INFO) << "Reading display dimensions";
      auto display_info = ConnectSyncIntoRealm<fuchsia_ui_display_singleton::Info>();

      auto res = display_info->GetMetrics();
      ZX_ASSERT_OK(res);

      display_width_ = res.value().info().extent_in_px()->width();
      display_height_ = res.value().info().extent_in_px()->height();

      FX_LOGS(INFO) << "Received display dimensions (" << display_width_ << ", " << display_height_
                    << ")";
    }

    fuchsia_ui_views::ViewCreationToken root_view_token;

    // Get root view token.
    {
      FX_LOGS(INFO) << "Creating root view token";

      auto controller = ConnectSyncIntoRealm<fuchsia_ui_test_scene::Controller>();

      fuchsia_ui_test_scene::ControllerPresentClientViewRequest req;
      auto [view_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
      req.viewport_creation_token(std::move(viewport_token));
      ZX_ASSERT_OK(controller->PresentClientView(std::move(req)));
      root_view_token = std::move(view_token);
    }

    {
      FX_LOGS(INFO) << "Create puppet under test";
      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(PUPPET_UNDER_TEST_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fuchsia_ui_input3::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      puppet_.client = fidl::SyncClient(std::move(puppet_client_end));
      auto mouse_listener_client_end = puppet_.mouse_listener.ServeAndGetClientEnd(dispatcher());

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.mouse_listener(std::move(mouse_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);
    }
  }

  void SimulateMouseEvent(std::vector<futi::MouseButton> pressed_buttons, int movement_x,
                          int movement_y) {
    FX_LOGS(INFO) << "Requesting mouse event";
    futi::MouseSimulateMouseEventRequest request;
    request.pressed_buttons(std::move(pressed_buttons));
    request.movement_x(movement_x);
    request.movement_y(movement_y);

    fake_mouse_->SimulateMouseEvent(request);
    FX_LOGS(INFO) << "Mouse event injected";
  }

  void ExpectEvent(const std::string& scoped_message,
                   const futi::MouseInputListenerReportMouseInputRequest& e, double expected_x,
                   double expected_y,
                   const std::vector<futi::MouseButton>& expected_buttons) const {
    SCOPED_TRACE(scoped_message);
    auto pixel_scale = e.device_pixel_ratio().has_value() ? e.device_pixel_ratio().value() : 1;
    EXPECT_NEAR(static_cast<double>(DevicePixelRatio()), pixel_scale, kEpsilon);
    auto actual_x = pixel_scale * e.local_x().value();
    auto actual_y = pixel_scale * e.local_y().value();
    EXPECT_NEAR(expected_x, actual_x, kPixelEpsilon);
    EXPECT_NEAR(expected_y, actual_y, kPixelEpsilon);
    if (expected_buttons.empty()) {
      EXPECT_FALSE(e.buttons().has_value());
    } else {
      ASSERT_TRUE(e.buttons().has_value());
      EXPECT_EQ(expected_buttons, e.buttons().value());
    }
    EXPECT_EQ(fake_mouse_device_id_, e.device_id().value());
  }

 protected:
  int32_t display_width_as_int() const { return static_cast<int32_t>(display_width_); }
  int32_t display_height_as_int() const { return static_cast<int32_t>(display_height_); }

  fidl::SyncClient<futc::PuppetFactory> puppet_factory_;

  fidl::SyncClient<futi::Mouse> fake_mouse_;
  uint32_t fake_mouse_device_id_ = 0;
  MousePuppet puppet_;
  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
};

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, MouseConformanceTest, ::zxtest::Values(1.0, 2.0));

TEST_P(MouseConformanceTest, SimpleClick) {
  // Inject click with no mouse movement.
  // Left button down.
  SimulateMouseEvent(/* pressed_buttons = */ {futi::MouseButton::kFirst},
                     /* movement_x = */ 0, /* movement_y = */ 0);
  FX_LOGS(INFO) << "Waiting for puppet to report DOWN event";
  RunLoopUntil([this]() { return this->puppet_.mouse_listener.events_received().size() >= 1; });

  ASSERT_EQ(puppet_.mouse_listener.events_received().size(), 1u);

  ExpectEvent("Down", puppet_.mouse_listener.events_received()[0],
              /* expected_x = */ static_cast<double>(display_width_) / 2.f,
              /* expected_y = */ static_cast<double>(display_height_) / 2.f,
              /* expected_buttons = */ {futi::MouseButton::kFirst});

  puppet_.mouse_listener.clear_events();

  // Left button up.
  SimulateMouseEvent(/* pressed_buttons = */ {}, /* movement_x = */ 0, /* movement_y = */ 0);

  FX_LOGS(INFO) << "Waiting for puppet to report UP";
  RunLoopUntil([this]() { return this->puppet_.mouse_listener.events_received().size() >= 1; });
  ASSERT_EQ(puppet_.mouse_listener.events_received().size(), 1u);

  ExpectEvent("Up", puppet_.mouse_listener.events_received()[0],
              /* expected_x = */ static_cast<double>(display_width_) / 2.f,
              /* expected_y = */ static_cast<double>(display_height_) / 2.f,
              /* expected_buttons = */ {});
}

}  //  namespace ui_conformance_testing
