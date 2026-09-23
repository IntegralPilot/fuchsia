
// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/testing/util/portable_ui_test.h"

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.input.virtualkeyboard/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.app/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <fidl/test.accessibility/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>

#include <utility>

#include <src/ui/testing/util/fidl_cpp_helpers.h>

namespace ui_testing {

namespace {

// Types imported for the realm_builder library.
using component_testing::ConfigValue;
using component_testing::Dictionary;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::RealmRoot;
using component_testing::Route;

bool CheckViewExistsInSnapshot(const fuchsia_ui_observation_geometry::ViewTreeSnapshot& snapshot,
                               zx_koid_t view_ref_koid) {
  if (!snapshot.views().has_value()) {
    return false;
  }

  auto snapshot_count = std::count_if(
      snapshot.views()->begin(), snapshot.views()->end(),
      [view_ref_koid](const auto& view) { return view.view_ref_koid() == view_ref_koid; });

  return snapshot_count > 0;
}

}  // namespace

void PortableUITest::SetUpRealmBase() {
  FX_LOGS(INFO) << "Setting up realm base.";

  // Add test UI stack component.
  realm_builder_.AddChild(kTestUIStack, GetTestUIStackUrl());

  // Route base system services to flutter and the test UI stack.
  realm_builder_.AddRoute(Route{
      .capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>},
                       Dictionary{"diagnostics"},
                       Protocol{fidl::DiscoverableProtocolName<fuchsia_scheduler::RoleManager>},
                       Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem::Allocator>},
                       Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem2::Allocator>},
                       Protocol{fidl::DiscoverableProtocolName<fuchsia_vulkan_loader::Loader>},
                       Protocol{
                           fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>}},
      .source = ParentRef{},
      .targets = {kTestUIStackRef}});

  // Route UI capabilities from test-ui-stack to test driver.
  realm_builder_.AddRoute(Route{
      .capabilities =
          {
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Screenshot>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_display_singleton::Info>},
              Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_ui_focus::FocusChainListenerRegistry>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_test_input::Registry>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_test_scene::Controller>},
              Protocol{fidl::DiscoverableProtocolName<test_accessibility::Magnifier>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_input_virtualkeyboard::Manager>},
              Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_input_virtualkeyboard::ControllerCreator>},
          },
      .source = kTestUIStackRef,
      .targets = {ParentRef{}}});

  // Configure test-ui-stack.
  realm_builder_.InitMutableConfigFromPackage(kTestUIStack);
  realm_builder_.SetConfigValue(kTestUIStack, "display_rotation",
                                ConfigValue::Uint32(display_rotation()));

  FX_CHECK((display_width_px() == 0) == (display_height_px() == 0))
      << "display_width_px() and display_height_px() must be either both zero or both non-zero.";
  realm_builder_.SetConfigValue(kTestUIStack, "display_width_px",
                                ConfigValue::Uint32(display_width_px()));
  realm_builder_.SetConfigValue(kTestUIStack, "display_height_px",
                                ConfigValue::Uint32(display_height_px()));
  realm_builder_.SetConfigValue(kTestUIStack, "display_refresh_rate_millihertz",
                                ConfigValue::Uint32(display_refresh_rate_millihertz()));

  realm_builder_.SetConfigValue(kTestUIStack, "device_pixel_ratio",
                                ConfigValue(std::to_string(device_pixel_ratio())));
  realm_builder_.SetConfigValue(kTestUIStack, "suspend_enabled",
                                ConfigValue::Bool(suspend_enabled()));
}

void PortableUITest::SetUp() {
  SetUpRealmBase();

  // Add additional components configured by the subclass.
  for (const auto& [name, component] : GetEagerTestComponents()) {
    realm_builder_.AddChild(
        name, component,
        component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER});
  }

  for (const auto& [name, component] : GetTestComponents()) {
    realm_builder_.AddChild(name, component);
  }

  ExtendRealm();

  // Add the necessary routing for each of the extra components added above, including any in
  // ExtendRealm.
  for (const auto& route : GetTestRoutes()) {
    realm_builder_.AddRoute(route);
  }

  realm_ = realm_builder_.Build();

  // Get the display dimensions. This is not only a log output, it ensures display info is ready.
  FX_LOGS(INFO) << "Got display_width = " << display_width()
                << " and display_height = " << display_height();
}

void PortableUITest::TearDown() {
  begin_tear_down_ = true;
  bool complete = false;
  realm_->Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
  RunLoopUntil([&]() { return complete; });
}

void PortableUITest::ProcessViewGeometryResponse(
    fuchsia_ui_observation_geometry::WatchResponse response) {
  // Process update if no error.
  if (!response.error().has_value()) {
    if (response.updates().has_value() && !response.updates()->empty()) {
      last_view_tree_snapshot_ = std::move(response.updates()->back());
    }
  } else {
    // Otherwise, process error.
    const auto& error = response.error().value();

    if (error & fuchsia_ui_observation_geometry::Error::kChannelOverflow) {
      FX_LOGS(DEBUG) << "View Tree watcher channel overflowed";
    } else if (error & fuchsia_ui_observation_geometry::Error::kBufferOverflow) {
      FX_LOGS(DEBUG) << "View Tree watcher buffer overflowed";
    } else if (error & fuchsia_ui_observation_geometry::Error::kViewsOverflow) {
      // This one indicates some possible data loss, so we log with a high severity.
      FX_LOGS(WARNING) << "View Tree watcher attempted to report too many views";
    }
  }
}

void PortableUITest::SetUpSceneProvider() {
  ASSERT_TRUE(realm_.has_value());
  auto scene_provider_connect = realm_->component().Connect<fuchsia_ui_test_scene::Controller>();
  ZX_ASSERT_OK(scene_provider_connect);
  scene_provider_ = fidl::SyncClient(std::move(scene_provider_connect.value()));
}

void PortableUITest::WatchViewGeometry() {
  FX_CHECK(view_tree_watcher_.is_valid())
      << "View Tree watcher must be registered before calling Watch()";

  view_tree_watcher_->Watch().Then([this](auto response) {
    if (!begin_tear_down_) {
      ZX_ASSERT_OK(response);
      ProcessViewGeometryResponse(std::move(response.value()));
      WatchViewGeometry();
    }
  });
}

void PortableUITest::WaitForViewPresentation() {
  SetUpSceneProvider();

  // WatchViewPresentation() will hang until a ClientView has `Present()`
  ZX_ASSERT_OK(scene_provider_->WatchViewPresentation());
}

bool PortableUITest::HasViewConnected(zx_koid_t view_ref_koid) {
  return last_view_tree_snapshot_.has_value() &&
         CheckViewExistsInSnapshot(*last_view_tree_snapshot_, view_ref_koid);
}

void PortableUITest::RegisterViewTreeWatcher() {
  auto [watcher_client, watcher_server] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  view_tree_watcher_ = fidl::Client(std::move(watcher_client), dispatcher());
  ZX_ASSERT_OK(scene_provider_->RegisterViewTreeWatcher({std::move(watcher_server)}));
}

void PortableUITest::LaunchClient() {
  SetUpSceneProvider();
  RegisterViewTreeWatcher();

  auto view_provider_connect = realm_->component().Connect<fuchsia_ui_app::ViewProvider>();
  ZX_ASSERT_OK(view_provider_connect);

  fuchsia_ui_test_scene::ControllerAttachClientViewRequest request;
  request.view_provider() = std::move(view_provider_connect.value());

  auto attach_client_view_res = scene_provider_->AttachClientView(std::move(request));
  ZX_ASSERT_OK(attach_client_view_res);

  client_root_view_ref_koid_ = attach_client_view_res->view_ref_koid();

  FX_LOGS(INFO) << "Waiting for client view ref koid";
  RunLoopUntil([this] { return client_root_view_ref_koid_.has_value(); });

  WatchViewGeometry();

  FX_LOGS(INFO) << "Waiting for client view to connect";
  RunLoopUntil([this] { return HasViewConnected(*client_root_view_ref_koid_); });
  FX_LOGS(INFO) << "Client view has rendered";
}

void PortableUITest::LaunchClientWithEmbeddedView() {
  LaunchClient();

  // At this point, the parent view must have rendered, so we just need to wait
  // for the embedded view.
  RunLoopUntil([this] {
    if (!last_view_tree_snapshot_.has_value() || !last_view_tree_snapshot_->views().has_value()) {
      return false;
    }

    if (!client_root_view_ref_koid_.has_value()) {
      return false;
    }

    for (const auto& view : last_view_tree_snapshot_->views().value()) {
      if (!view.view_ref_koid().has_value() ||
          view.view_ref_koid().value() != *client_root_view_ref_koid_) {
        continue;
      }

      if (view.children()->empty()) {
        return false;
      }

      // NOTE: We can't rely on the presence of the child view in
      // `view.children()` to guarantee that it has rendered. The child view
      // also needs to be present in `last_view_tree_snapshot_->views`.
      return std::count_if(last_view_tree_snapshot_->views()->begin(),
                           last_view_tree_snapshot_->views()->end(),
                           [view_to_find = view.children()->back()](const auto& view_to_check) {
                             return view_to_check.view_ref_koid().has_value() &&
                                    view_to_check.view_ref_koid().value() == view_to_find;
                           }) > 0;
    }

    return false;
  });

  FX_LOGS(INFO) << "Embedded view has rendered";
}

Screenshot PortableUITest::TakeScreenshot(ScreenshotFormat format) {
  if (!screenshotter_.is_valid()) {
    auto connect = realm_root()->component().Connect<fuchsia_ui_composition::Screenshot>();
    ZX_ASSERT_OK(connect);
    screenshotter_ = fidl::SyncClient(std::move(connect.value()));
  }
  FX_LOGS(INFO) << "Taking screenshot... ";

  auto res = screenshotter_->Take({{.format = format}});
  ZX_ASSERT_OK(res);

  FX_LOGS(INFO) << "Screenshot captured.";

  if (format == ScreenshotFormat::kPng) {
    return Screenshot(res->vmo().value());
  }
  return Screenshot(res->vmo().value(), display_size().width(), display_size().height(),
                    display_rotation());
}

bool PortableUITest::TakeScreenshotUntil(
    fit::function<bool(const Screenshot&)> screenshot_predicate, zx::duration predicate_timeout,
    zx::duration step, ScreenshotFormat format) {
  return RunLoopWithTimeoutOrUntil(
      [this, &screenshot_predicate, &format] {
        return screenshot_predicate(TakeScreenshot(format));
      },
      predicate_timeout, step);
}

fuchsia_math::SizeU PortableUITest::display_size() {
  if (display_size_)
    return display_size_.value();

  auto display_info_connect =
      realm_root()->component().Connect<fuchsia_ui_display_singleton::Info>();
  ZX_ASSERT_OK(display_info_connect);
  fidl::SyncClient<fuchsia_ui_display_singleton::Info> display_info(
      std::move(display_info_connect.value()));
  auto get_metrics_res = display_info->GetMetrics();
  ZX_ASSERT_OK(get_metrics_res);

  display_size_ = get_metrics_res->info().extent_in_px();
  return display_size_.value();
}

uint32_t PortableUITest::display_width() { return display_size().width(); }

uint32_t PortableUITest::display_height() { return display_size().height(); }

void PortableUITest::RegisterTouchScreen() {
  FX_LOGS(INFO) << "Registering fake touch screen";
  ConnectInputRegistry();

  auto [register_touch_client, register_touch_server] =
      fidl::Endpoints<fuchsia_ui_test_input::TouchScreen>::Create();
  fake_touchscreen_ = fidl::SyncClient(std::move(register_touch_client));

  ZX_ASSERT_OK(input_registry_->RegisterTouchScreen(
      {{.device = std::move(register_touch_server),
        .coordinate_unit = fuchsia_ui_test_input::CoordinateUnit::kPhysicalPixels}}));
  FX_LOGS(INFO) << "Touchscreen registered";
}

void PortableUITest::InjectTap(int32_t x, int32_t y) {
  fuchsia_ui_test_input::TouchScreenSimulateTapRequest tap_request;
  tap_request.tap_location() = fuchsia_math::Vec{{.x = x, .y = y}};

  FX_LOGS(INFO) << "Injecting tap at (" << tap_request.tap_location()->x() << ", "
                << tap_request.tap_location()->y() << ")";
  fake_touchscreen_->SimulateTap(tap_request);
  ++touch_injection_request_count_;
  FX_LOGS(INFO) << "*** Tap injected, count: " << touch_injection_request_count_;
}

void PortableUITest::InjectTapWithRetry(int32_t x, int32_t y) {
  InjectTap(x, y);
  async::PostDelayedTask(
      dispatcher(), [this, x, y] { InjectTapWithRetry(x, y); }, kTapRetryInterval);
}

void PortableUITest::InjectSwipe(int start_x, int start_y, int end_x, int end_y,
                                 int move_event_count) {
  fuchsia_ui_test_input::TouchScreenSimulateSwipeRequest swipe_request;
  swipe_request.start_location() = {{.x = start_x, .y = start_y}};
  swipe_request.end_location() = {{.x = end_x, .y = end_y}};
  swipe_request.move_event_count() = move_event_count;

  FX_LOGS(INFO) << "Injecting swipe from (" << swipe_request.start_location()->x() << ", "
                << swipe_request.start_location()->y() << ") to ("
                << swipe_request.end_location()->x() << ", " << swipe_request.end_location()->y()
                << ") with move_event_count = " << swipe_request.move_event_count().value();

  fake_touchscreen_->SimulateSwipe(swipe_request);
  touch_injection_request_count_++;
  FX_LOGS(INFO) << "*** Swipe injected";
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
void PortableUITest::InjectTouchEvent(fuchsia_ui_test_input::TouchInputReport report) {
  fuchsia_ui_test_input::TouchScreenSimulateTouchEventRequest request;
  request.report() = std::move(report);

  fake_touchscreen_->SimulateTouchEvent(request);
  touch_injection_request_count_++;
  FX_LOGS(INFO) << "*** Touch event injected";
}
#else
void PortableUITest::InjectTouchEvent(fuchsia_input_report::TouchInputReport report) {
  fuchsia_ui_test_input::TouchScreenSimulateTouchEventRequest request;
  request.report() = std::move(report);

  fake_touchscreen_->SimulateTouchEvent(request);
  touch_injection_request_count_++;
  FX_LOGS(INFO) << "*** Touch event injected";
}
#endif

void PortableUITest::RegisterMouse() {
  FX_LOGS(INFO) << "Registering fake mouse";
  ConnectInputRegistry();

  auto [register_mouse_client, register_mouse_server] =
      fidl::Endpoints<fuchsia_ui_test_input::Mouse>::Create();
  fake_mouse_ = fidl::SyncClient(std::move(register_mouse_client));

  ZX_ASSERT_OK(input_registry_->RegisterMouse({{.device = std::move(register_mouse_server)}}));
  FX_LOGS(INFO) << "Mouse registered";
}

void PortableUITest::SimulateMouseEvent(
    const std::vector<fuchsia_ui_test_input::MouseButton>& pressed_buttons, int movement_x,
    int movement_y) {
  FX_LOGS(INFO) << "Requesting mouse event";

  ZX_ASSERT_OK(fake_mouse_->SimulateMouseEvent(
      {{.pressed_buttons = pressed_buttons, .movement_x = movement_x, .movement_y = movement_y}}));
}

void PortableUITest::SimulateMouseScroll(
    const std::vector<fuchsia_ui_test_input::MouseButton>& pressed_buttons, int scroll_x,
    int scroll_y, bool use_physical_units) {
  FX_LOGS(INFO) << "Requesting mouse scroll";

  fuchsia_ui_test_input::MouseSimulateMouseEventRequest request({
      .pressed_buttons = pressed_buttons,
  });

  if (use_physical_units) {
    request.scroll_h_physical_pixel() = scroll_x;
    request.scroll_v_physical_pixel() = scroll_y;
  } else {
    request.scroll_h_detent() = scroll_x;
    request.scroll_v_detent() = scroll_y;
  }

  ZX_ASSERT_OK(fake_mouse_->SimulateMouseEvent(request));
}

void PortableUITest::RegisterKeyboard() {
  FX_LOGS(INFO) << "Registering fake keyboard";
  ConnectInputRegistry();

  auto [register_kb_client, register_kb_server] =
      fidl::Endpoints<fuchsia_ui_test_input::Keyboard>::Create();
  fake_keyboard_ = fidl::SyncClient(std::move(register_kb_client));

  ZX_ASSERT_OK(input_registry_->RegisterKeyboard({{.device = std::move(register_kb_server)}}));
  FX_LOGS(INFO) << "Keyboard registered";
}

void PortableUITest::SimulateUsAsciiTextEntry(const std::string& str) {
  FX_LOGS(INFO) << "Requesting key event";

  ZX_ASSERT_OK(fake_keyboard_->SimulateUsAsciiTextEntry({{.text = str}}));
}

void PortableUITest::ConnectInputRegistry() {
  if (!input_registry_.is_valid()) {
    auto res = realm_->component().Connect<fuchsia_ui_test_input::Registry>();
    ZX_ASSERT_OK(res);
    input_registry_ = fidl::SyncClient(std::move(res.value()));
  }
}

}  // namespace ui_testing
