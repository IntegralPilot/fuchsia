// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.conformance/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>

#include <algorithm>
#include <cstdint>
#include <string>
#include <utility>
#include <vector>

#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/zxtest_helpers.h>
#include <src/ui/tests/conformance_input_tests/conformance-test-base.h>
#include <zxtest/zxtest.h>

namespace ui_conformance_testing {

namespace futi = fuchsia_ui_test_input;
namespace fir = fuchsia_input_report;
namespace futc = fuchsia_ui_test_conformance;
namespace fup = fuchsia_ui_pointer;

const std::string PUPPET_UNDER_TEST_FACTORY_SERVICE = "/svc/puppet-under-test-factory-service";
const std::string AUXILIARY_PUPPET_FACTORY_SERVICE = "/svc/auxiliary-puppet-factory-service";

// Two physical pixel coordinates are considered equivalent if their distance is less than 1 unit.
constexpr double kPixelEpsilon = 0.5f;

// Epsilon for floating error.
constexpr double kEpsilon = 0.0001f;

// Input stack does not guarantee the order of pointer in 1 contact report.
// So tests sort the request vector by pointer id and keep the order of
// event time.
void SortTouchInputRequestByTimeAndTimestampAndPointerId(
    std::vector<futi::TouchInputListenerReportTouchInputRequest>& requests) {
  std::sort(requests.begin(), requests.end(), [](const auto& a, const auto& b) {
    if (a.time_received() != b.time_received()) {
      return a.time_received() < b.time_received();
    }
    return a.pointer_id() < b.pointer_id();
  });
}

class TouchListener : public fidl::Server<futi::TouchInputListener> {
 public:
  // |futi::TouchInputListener|
  void ReportTouchInput(ReportTouchInputRequest& request,
                        ReportTouchInputCompleter::Sync& completer) override {
    events_received_.push_back(std::move(request));
  }

  fidl::ClientEnd<futi::TouchInputListener> ServeAndGetClientEnd(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<futi::TouchInputListener>::Create();
    binding_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  const std::vector<futi::TouchInputListenerReportTouchInputRequest>& events_received() {
    return events_received_;
  }

  std::vector<futi::TouchInputListenerReportTouchInputRequest> cloned_events_received() {
    auto res = std::vector<futi::TouchInputListenerReportTouchInputRequest>();
    for (auto& req : events_received_) {
      futi::TouchInputListenerReportTouchInputRequest clone(req);
      res.push_back(std::move(clone));
    }

    return res;
  }

  void clear_events() { events_received_.clear(); }

  bool LastEventReceivedMatchesPhase(fup::EventPhase phase) {
    if (events_received_.empty()) {
      return false;
    }

    const auto& last_event = events_received_.back();
    const auto actual_phase = last_event.phase();

    FX_LOGS(INFO) << "Expecting event at phase (" << static_cast<uint32_t>(phase) << ")";
    FX_LOGS(INFO) << "Received event at phase (" << static_cast<uint32_t>(actual_phase.value())
                  << ")";

    return phase == actual_phase;
  }

 private:
  fidl::ServerBindingGroup<futi::TouchInputListener> binding_;
  std::vector<futi::TouchInputListenerReportTouchInputRequest> events_received_;
};

// Holds resources associated with a single puppet instance.
struct TouchPuppet {
  fidl::SyncClient<futc::Puppet> client;
  TouchListener touch_listener;
};

using device_pixel_ratio = float;

class TouchConformanceTest : public ui_conformance_test_base::ConformanceTest,
                             public zxtest::WithParamInterface<device_pixel_ratio> {
 public:
  ~TouchConformanceTest() override = default;

  float DevicePixelRatio() const override { return GetParam(); }

  void SetUp() override {
    ui_conformance_test_base::ConformanceTest::SetUp();

    // Register fake touch screen.
    {
      FX_LOGS(INFO) << "Connecting to input registry";
      auto input_registry = ConnectSyncIntoRealm<futi::Registry>();

      FX_LOGS(INFO) << "Registering fake touch screen";
      auto [client_end, server_end] = fidl::Endpoints<futi::TouchScreen>::Create();
      fake_touch_screen_ = fidl::SyncClient(std::move(client_end));

      futi::RegistryRegisterTouchScreenAndGetDeviceInfoRequest request;
      request.device(std::move(server_end));
      request.coordinate_unit(futi::CoordinateUnit::kPhysicalPixels);
      auto res = input_registry->RegisterTouchScreenAndGetDeviceInfo(std::move(request));
      ZX_ASSERT_OK(res);
      fake_touch_screen_device_id_ = res->device_id().value();
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
  }

  // TODO(https://fxbug.dev/42076606): Two coordinates (x/y) systems can differ in scale (size of
  // pixels).
  void ExpectLocationAndPhase(const std::string& scoped_message,
                              const futi::TouchInputListenerReportTouchInputRequest& e,
                              float expected_pixel_ratio, double expected_x, double expected_y,
                              fup::EventPhase expected_phase,
                              const uint32_t expected_pointer_id) const {
    SCOPED_TRACE(scoped_message);
    auto pixel_scale = e.device_pixel_ratio().has_value() ? e.device_pixel_ratio().value() : 1;
    EXPECT_NEAR(static_cast<double>(expected_pixel_ratio), pixel_scale, kEpsilon);
    auto actual_x = pixel_scale * e.local_x().value();
    auto actual_y = pixel_scale * e.local_y().value();
    EXPECT_NEAR(expected_x, actual_x, kPixelEpsilon);
    EXPECT_NEAR(expected_y, actual_y, kPixelEpsilon);
    EXPECT_EQ(expected_phase, e.phase());
    EXPECT_EQ(expected_pointer_id, e.pointer_id());
    EXPECT_EQ(fake_touch_screen_device_id_, e.device_id().value());
  }

 protected:
  int32_t display_width_as_int() const { return static_cast<int32_t>(display_width_); }
  int32_t display_height_as_int() const { return static_cast<int32_t>(display_height_); }

  fidl::SyncClient<futi::TouchScreen> fake_touch_screen_;
  uint32_t fake_touch_screen_device_id_;
  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
};

class SingleViewTouchConformanceTest : public TouchConformanceTest {
 public:
  ~SingleViewTouchConformanceTest() override = default;

  void SetUp() override {
    TouchConformanceTest::SetUp();

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

      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      puppet_.client = fidl::SyncClient(std::move(puppet_client_end));
      auto touch_listener_client_end = puppet_.touch_listener.ServeAndGetClientEnd(dispatcher());
      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fuchsia_ui_input3::Keyboard>();

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.touch_listener(std::move(touch_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);
    }
  }

 protected:
  TouchPuppet puppet_;

 private:
  fidl::SyncClient<futc::PuppetFactory> puppet_factory_;
};

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, SingleViewTouchConformanceTest, ::zxtest::Values(1.0, 2.0));

TEST_P(SingleViewTouchConformanceTest, SimpleTap) {
  const auto kTapX = 3 * display_width_as_int() / 4;
  const auto kTapY = display_height_as_int() / 4;

  // Inject tap in the middle of the top-right quadrant.
  futi::TouchScreenSimulateTapRequest tap_request;
  tap_request.tap_location() = {kTapX, kTapY};
  FX_LOGS(INFO) << "Injecting tap at (" << kTapX << ", " << kTapY << ")";
  fake_touch_screen_->SimulateTap(tap_request);

  FX_LOGS(INFO) << "Waiting for touch event listener to receive response";
  RunLoopUntil([&]() {
    return puppet_.touch_listener.LastEventReceivedMatchesPhase(fup::EventPhase::kRemove);
  });

  const auto& events_received = puppet_.touch_listener.events_received();
  ASSERT_EQ(events_received.size(), 2u);
  // The puppet's view matches the display dimensions exactly, and the device
  // pixel ratio is 1. Therefore, the puppet's logical coordinate space will
  // match the physical coordinate space, so we expect the puppet to report
  // the event at (kTapX, kTapY).
  ExpectLocationAndPhase("[0]", events_received[0], DevicePixelRatio(), kTapX, kTapY,
                         fup::EventPhase::kAdd, 1);
  ExpectLocationAndPhase("[1]", events_received[1], DevicePixelRatio(), kTapX, kTapY,
                         fup::EventPhase::kRemove, 1);
}

// Send input report contains 3 contacts and then release all of them to
// simultate multi touch down on touchscreen. UI client expects 3 touch
// add events, and 3 touch remove events.
TEST_P(SingleViewTouchConformanceTest, MultiTap) {
  const int kTap1X = 3 * display_width_as_int() / 4;
  const int kTapY = display_height_as_int() / 4;
  const int kTap0X = kTap1X - 5;
  const int kTap2X = kTap1X + 5;

  futi::TouchScreenSimulateMultiTapRequest multi_tap_req;
  multi_tap_req.tap_locations() = {{kTap0X, kTapY}, {kTap1X, kTapY}, {kTap2X, kTapY}};
  FX_LOGS(INFO) << "Injecting tap at (" << kTap0X << ", " << kTapY << ") (" << kTap1X << ", "
                << kTapY << ") (" << kTap2X << ", " << kTapY << ")";
  fake_touch_screen_->SimulateMultiTap(multi_tap_req);

  FX_LOGS(INFO) << "Waiting for touch event listener to receive response";

  // There are 3 * touch down and 3 * touch remove, totally 6 events.
  const size_t kExpectedEventCounts = 6;
  RunLoopUntil(
      [&]() { return puppet_.touch_listener.events_received().size() >= kExpectedEventCounts; });

  auto events_received = puppet_.touch_listener.cloned_events_received();
  ASSERT_EQ(events_received.size(), kExpectedEventCounts);

  SortTouchInputRequestByTimeAndTimestampAndPointerId(events_received);

  ExpectLocationAndPhase("add [0]", events_received[0], DevicePixelRatio(), kTap0X, kTapY,
                         fup::EventPhase::kAdd, 0);
  ExpectLocationAndPhase("add [1]", events_received[1], DevicePixelRatio(), kTap1X, kTapY,
                         fup::EventPhase::kAdd, 1);
  ExpectLocationAndPhase("add [2]", events_received[2], DevicePixelRatio(), kTap2X, kTapY,
                         fup::EventPhase::kAdd, 2);

  ExpectLocationAndPhase("remove [0]", events_received[3], DevicePixelRatio(), kTap0X, kTapY,
                         fup::EventPhase::kRemove, 0);
  ExpectLocationAndPhase("remove [1]", events_received[4], DevicePixelRatio(), kTap1X, kTapY,
                         fup::EventPhase::kRemove, 1);
  ExpectLocationAndPhase("remove [2]", events_received[5], DevicePixelRatio(), kTap2X, kTapY,
                         fup::EventPhase::kRemove, 2);
}

// 2 finger contact then move apart horizontally to simulate pinch zoom.
// UI Client expects touch add, touch change and touch remove for
// 2 fingers.
TEST_P(SingleViewTouchConformanceTest, Pinch) {
  const int kTapX = 3 * display_width_as_int() / 4;
  const int kTapY = display_height_as_int() / 4;

  futi::TouchScreenSimulateMultiFingerGestureRequest pinch_req;
  pinch_req.start_locations() = {fuchsia_math::Vec(kTapX - 5, kTapY),
                                 fuchsia_math::Vec(kTapX + 5, kTapY)};
  pinch_req.end_locations() = {fuchsia_math::Vec(kTapX - 20, kTapY),
                               fuchsia_math::Vec(kTapX + 20, kTapY)};
  pinch_req.move_event_count(3);
  pinch_req.finger_count(2);
  FX_LOGS(INFO) << "Injecting pinch from (" << pinch_req.start_locations().value()[0].x() << ", "
                << pinch_req.start_locations().value()[0].y() << ") ("
                << pinch_req.start_locations().value()[1].x() << ", "
                << pinch_req.start_locations().value()[1].y() << ") to ("
                << pinch_req.end_locations().value()[0].x() << ", "
                << pinch_req.end_locations().value()[0].y() << ") ("
                << pinch_req.end_locations().value()[1].x() << ", "
                << pinch_req.end_locations().value()[1].y()
                << ") with move_event_count = " << pinch_req.move_event_count().value();
  fake_touch_screen_->SimulateMultiFingerGesture(pinch_req);

  FX_LOGS(INFO) << "Waiting for touch event listener to receive response";

  // There are 2 * touch down + 4 * touch change + 2 * touch remove, totally 8 events.
  const size_t kExpectedEventCounts = 8;

  RunLoopUntil(
      [&]() { return puppet_.touch_listener.events_received().size() >= kExpectedEventCounts; });

  auto events_received = puppet_.touch_listener.cloned_events_received();
  ASSERT_EQ(events_received.size(), kExpectedEventCounts);

  SortTouchInputRequestByTimeAndTimestampAndPointerId(events_received);

  ExpectLocationAndPhase("add [0]", events_received[0], DevicePixelRatio(), kTapX - 5, kTapY,
                         fup::EventPhase::kAdd, 0);
  ExpectLocationAndPhase("add [1]", events_received[1], DevicePixelRatio(), kTapX + 5, kTapY,
                         fup::EventPhase::kAdd, 1);

  ExpectLocationAndPhase("change [0] 1", events_received[2], DevicePixelRatio(), kTapX - 10, kTapY,
                         fup::EventPhase::kChange, 0);
  ExpectLocationAndPhase("change [1] 1", events_received[3], DevicePixelRatio(), kTapX + 10, kTapY,
                         fup::EventPhase::kChange, 1);

  ExpectLocationAndPhase("change [0] 2", events_received[4], DevicePixelRatio(), kTapX - 15, kTapY,
                         fup::EventPhase::kChange, 0);
  ExpectLocationAndPhase("change [1] 2", events_received[5], DevicePixelRatio(), kTapX + 15, kTapY,
                         fup::EventPhase::kChange, 1);

  ExpectLocationAndPhase("remove [1]", events_received[6], DevicePixelRatio(), kTapX - 15, kTapY,
                         fup::EventPhase::kRemove, 0);
  ExpectLocationAndPhase("remove [2]", events_received[7], DevicePixelRatio(), kTapX + 15, kTapY,
                         fup::EventPhase::kRemove, 1);
}

TEST_P(SingleViewTouchConformanceTest, TouchEventFields) {
  const uint32_t kID = 1u;
  const int kX = 3 * display_width_as_int() / 4;
  const int kY = display_height_as_int() / 4;
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
  fuchsia_ui_test_input::ContactInputReport contact;
  contact.contact_id(kID);
  contact.position_x(kX);
  contact.position_y(kY);

  fuchsia_ui_test_input::TouchInputReport report;
  report.contacts() = {std::move(contact)};
#else
  const int64_t kHeight = 10;
  const int64_t kWidth = 15;
  const int64_t kPressure = 20;

  fir::ContactInputReport contact;
  contact.contact_id(kID);
  contact.position_x(kX);
  contact.position_y(kY);

  // following fields are not passing to UI client yet.
  contact.contact_height(kHeight);
  contact.contact_width(kWidth);
  contact.confidence(true);
  contact.pressure(kPressure);

  fir::TouchInputReport report;
  report.contacts() = {std::move(contact)};
#endif

  fake_touch_screen_->SimulateTouchEvent(std::move(report));

  RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 1; });

  auto events_received = puppet_.touch_listener.cloned_events_received();
  ASSERT_EQ(events_received.size(), 1u);

  ExpectLocationAndPhase("add [1]", events_received[0], DevicePixelRatio(), kX, kY,
                         fup::EventPhase::kAdd, kID);

  puppet_.touch_listener.clear_events();
}

// The test wants to ensure UI client will receive events in order:
// finger 1 down:
// 1. finger 1 add
//
// finger 2 down:
// 1. finger 1 change
// 2. finger 2 add
//
// keep 2 fingers down:
// finger 1/2 change <- in any order
//
// finger 2 up:
// 1. finger 1 update
// 2. finger 2 remove
//
// finger 1 up：
// 1. finger 1 remove
TEST_P(SingleViewTouchConformanceTest, OneFingerDownThenAnotherThenLift) {
  const int kFinger1X = 3 * display_width_as_int() / 4;
  const int kY = display_height_as_int() / 4;
  const int kFinger2X = kFinger1X + 5;

  auto build_contact = [](uint32_t id, int64_t x, int64_t y) {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::ContactInputReport contact;
#else
    fir::ContactInputReport contact;
#endif
    contact.contact_id(id);
    contact.position_x(x);
    contact.position_y(y);
    return contact;
  };

  {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::TouchInputReport finger1_down;
#else
    fir::TouchInputReport finger1_down;
#endif
    finger1_down.contacts() = {build_contact(1u, kFinger1X, kY)};

    fake_touch_screen_->SimulateTouchEvent(std::move(finger1_down));

    RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 1; });

    auto events_received = puppet_.touch_listener.cloned_events_received();
    ASSERT_EQ(events_received.size(), 1u);

    ExpectLocationAndPhase("add [1]", events_received[0], DevicePixelRatio(), kFinger1X, kY,
                           fup::EventPhase::kAdd, 1);

    puppet_.touch_listener.clear_events();
  }

  {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::TouchInputReport finger_1_2_down;
#else
    fir::TouchInputReport finger_1_2_down;
#endif
    finger_1_2_down.contacts() = {
        build_contact(1u, kFinger1X, kY),
        build_contact(2u, kFinger2X, kY),
    };

    fake_touch_screen_->SimulateTouchEvent(std::move(finger_1_2_down));

    RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 2; });

    auto events_received = puppet_.touch_listener.cloned_events_received();
    ASSERT_EQ(events_received.size(), 2u);

    ExpectLocationAndPhase("change [1]", events_received[0], DevicePixelRatio(), kFinger1X, kY,
                           fup::EventPhase::kChange, 1);
    ExpectLocationAndPhase("add [2]", events_received[1], DevicePixelRatio(), kFinger2X, kY,
                           fup::EventPhase::kAdd, 2);

    puppet_.touch_listener.clear_events();
  }

  {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::TouchInputReport keep_finger_1_2_down;
#else
    fir::TouchInputReport keep_finger_1_2_down;
#endif
    keep_finger_1_2_down.contacts() = {
        build_contact(1u, kFinger1X, kY),
        build_contact(2u, kFinger2X, kY),
    };

    fake_touch_screen_->SimulateTouchEvent(std::move(keep_finger_1_2_down));

    RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 2; });

    auto events_received = puppet_.touch_listener.cloned_events_received();
    ASSERT_EQ(events_received.size(), 2u);

    std::sort(events_received.begin(), events_received.end(),
              [](const auto& a, const auto& b) { return a.pointer_id() < b.pointer_id(); });
    ExpectLocationAndPhase("change [1]", events_received[0], DevicePixelRatio(), kFinger1X, kY,
                           fup::EventPhase::kChange, 1);
    ExpectLocationAndPhase("change [2]", events_received[1], DevicePixelRatio(), kFinger2X, kY,
                           fup::EventPhase::kChange, 2);

    puppet_.touch_listener.clear_events();
  }

  {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::TouchInputReport finger_2_lift;
#else
    fir::TouchInputReport finger_2_lift;
#endif
    finger_2_lift.contacts() = {build_contact(1u, kFinger1X, kY)};

    fake_touch_screen_->SimulateTouchEvent(std::move(finger_2_lift));

    RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 2; });

    auto events_received = puppet_.touch_listener.cloned_events_received();
    ASSERT_EQ(events_received.size(), 2u);

    ExpectLocationAndPhase("change [1]", events_received[0], DevicePixelRatio(), kFinger1X, kY,
                           fup::EventPhase::kChange, 1);
    ExpectLocationAndPhase("remove [2]", events_received[1], DevicePixelRatio(), kFinger2X, kY,
                           fup::EventPhase::kRemove, 2);

    puppet_.touch_listener.clear_events();
  }

  {
#if FUCHSIA_API_LEVEL_AT_LEAST(32)
    fuchsia_ui_test_input::TouchInputReport finger_1_lift;
#else
    fir::TouchInputReport finger_1_lift;
#endif

    fake_touch_screen_->SimulateTouchEvent(std::move(finger_1_lift));

    RunLoopUntil([&]() { return puppet_.touch_listener.events_received().size() >= 1; });

    auto events_received = puppet_.touch_listener.cloned_events_received();
    ASSERT_EQ(events_received.size(), 1u);

    ExpectLocationAndPhase("remove [1]", events_received[0], DevicePixelRatio(), kFinger1X, kY,
                           fup::EventPhase::kRemove, 1);

    puppet_.touch_listener.clear_events();
  }
}

class EmbeddedViewTouchConformanceTest : public TouchConformanceTest {
 public:
  void SetUp() override {
    TouchConformanceTest::SetUp();

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
      FX_LOGS(INFO) << "Create parent puppet";

      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(AUXILIARY_PUPPET_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      parent_puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      parent_puppet_.client = fidl::SyncClient(std::move(puppet_client_end));
      auto touch_listener_client_end =
          parent_puppet_.touch_listener.ServeAndGetClientEnd(dispatcher());
      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fuchsia_ui_input3::Keyboard>();

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.touch_listener(std::move(touch_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = parent_puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);
    }

    // Create child viewport.
    fuchsia_ui_views::ViewCreationToken view_creation_token;
    {
      FX_LOGS(INFO) << "Creating child viewport";
      const uint64_t kChildViewportId = 1u;

      futc::ContentBounds bounds;
      bounds.size() = {
          static_cast<uint32_t>(static_cast<float>(display_width_) / DevicePixelRatio() / 2.0),
          static_cast<uint32_t>(static_cast<float>(display_height_) / DevicePixelRatio() / 2.0)};
      bounds.origin() = {
          static_cast<int32_t>(static_cast<float>(display_width_) / DevicePixelRatio() / 2.0),
          static_cast<int32_t>(static_cast<float>(display_height_) / DevicePixelRatio() / 2.0),
      };
      futc::EmbeddedViewProperties properties({
          .bounds = std::move(bounds),
      });
      futc::PuppetEmbedRemoteViewRequest embed_remote_view_request({
          .id = kChildViewportId,
          .properties = std::move(properties),
      });
      auto res = parent_puppet_.client->EmbedRemoteView(embed_remote_view_request);
      ZX_ASSERT_OK(res);
      view_creation_token = std::move(res->view_creation_token()->value());
    }

    // Create child view.
    {
      FX_LOGS(INFO) << "Creating child puppet";
      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(AUXILIARY_PUPPET_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      child_puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      child_puppet_.client = fidl::SyncClient(std::move(puppet_client_end));
      auto touch_listener_client_end =
          child_puppet_.touch_listener.ServeAndGetClientEnd(dispatcher());
      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fuchsia_ui_input3::Keyboard>();

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(view_creation_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.touch_listener(std::move(touch_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = child_puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);
    }
  }

 protected:
  TouchPuppet parent_puppet_;
  TouchPuppet child_puppet_;

 private:
  fidl::SyncClient<futc::PuppetFactory> parent_puppet_factory_;
  fidl::SyncClient<futc::PuppetFactory> child_puppet_factory_;
};

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, EmbeddedViewTouchConformanceTest,
                         ::zxtest::Values(1.0, 2.0));

TEST_P(EmbeddedViewTouchConformanceTest, EmbeddedViewTap) {
  const auto kTapX = 3 * display_width_as_int() / 4;
  const auto kTapY = 3 * display_height_as_int() / 4;

  // Inject tap in the middle of the bottom-right quadrant.
  futi::TouchScreenSimulateTapRequest tap_request;
  tap_request.tap_location() = {kTapX, kTapY};
  FX_LOGS(INFO) << "Injecting tap at (" << kTapX << ", " << kTapY << ")";
  fake_touch_screen_->SimulateTap(tap_request);

  FX_LOGS(INFO) << "Waiting for child touch event listener to receive response";

  RunLoopUntil([&]() {
    return child_puppet_.touch_listener.LastEventReceivedMatchesPhase(fup::EventPhase::kRemove);
  });

  auto& events_received = child_puppet_.touch_listener.events_received();
  ASSERT_EQ(events_received.size(), 2u);

  // The child view's origin is in the center of the screen, so the center of
  // the bottom-right quadrant is at (display_width_ / 4, display_height_ / 4)
  // in the child's local coordinate space.
  const double quarter_display_width = static_cast<double>(display_width_) / 4.f;
  const double quarter_display_height = static_cast<double>(display_height_) / 4.f;
  ExpectLocationAndPhase("[0]", events_received[0], DevicePixelRatio(), quarter_display_width,
                         quarter_display_height, fup::EventPhase::kAdd, 1);
  ExpectLocationAndPhase("[1]", events_received[1], DevicePixelRatio(), quarter_display_width,
                         quarter_display_height, fup::EventPhase::kRemove, 1);

  // The parent should not have received any pointer events.
  EXPECT_TRUE(parent_puppet_.touch_listener.events_received().empty());
}

// Send input report contains 3 contacts and then release all of them to
// simultate multi touch down on touchscreen. Embedded view expects 3 touch
// add events, and 3 touch remove events. And no events on parenet view.
TEST_P(EmbeddedViewTouchConformanceTest, EmbeddedViewMultiTap) {
  const int kTap1X = 3 * display_width_as_int() / 4;
  const int kTapY = 3 * display_height_as_int() / 4;
  const int kTap0X = kTap1X - 5;
  const int kTap2X = kTap1X + 5;

  futi::TouchScreenSimulateMultiTapRequest multi_tap_req;
  multi_tap_req.tap_locations() = {{kTap0X, kTapY}, {kTap1X, kTapY}, {kTap2X, kTapY}};
  FX_LOGS(INFO) << "Injecting tap at (" << kTap0X << ", " << kTapY << ") (" << kTap1X << ", "
                << kTapY << ") (" << kTap2X << ", " << kTapY << ")";
  fake_touch_screen_->SimulateMultiTap(multi_tap_req);

  FX_LOGS(INFO) << "Waiting for touch event listener to receive response";
  // There are 3 * touch down and 3 * touch remove, totally 6 events.
  const size_t kExpectedEventCounts = 6;

  RunLoopUntil([&]() {
    return child_puppet_.touch_listener.events_received().size() >= kExpectedEventCounts;
  });

  auto events_received = child_puppet_.touch_listener.cloned_events_received();
  ASSERT_EQ(events_received.size(), kExpectedEventCounts);

  const double kTap1XChildView = static_cast<double>(display_width_) / 4.f;
  const double kTapYChildView = static_cast<double>(display_height_) / 4.f;
  const double kTap0XChildView = kTap1XChildView - 5;
  const double kTap2XChildView = kTap1XChildView + 5;

  SortTouchInputRequestByTimeAndTimestampAndPointerId(events_received);

  ExpectLocationAndPhase("add [0]", events_received[0], DevicePixelRatio(), kTap0XChildView,
                         kTapYChildView, fup::EventPhase::kAdd, 0);
  ExpectLocationAndPhase("add [1]", events_received[1], DevicePixelRatio(), kTap1XChildView,
                         kTapYChildView, fup::EventPhase::kAdd, 1);
  ExpectLocationAndPhase("add [2]", events_received[2], DevicePixelRatio(), kTap2XChildView,
                         kTapYChildView, fup::EventPhase::kAdd, 2);

  ExpectLocationAndPhase("remove [1]", events_received[3], DevicePixelRatio(), kTap0XChildView,
                         kTapYChildView, fup::EventPhase::kRemove, 0);
  ExpectLocationAndPhase("remove [2]", events_received[4], DevicePixelRatio(), kTap1XChildView,
                         kTapYChildView, fup::EventPhase::kRemove, 1);
  ExpectLocationAndPhase("remove [3]", events_received[5], DevicePixelRatio(), kTap2XChildView,
                         kTapYChildView, fup::EventPhase::kRemove, 2);

  // The parent should not have received any pointer events.
  EXPECT_TRUE(parent_puppet_.touch_listener.events_received().empty());
}

// 2 finger contact then move apart horizontally to simulate pinch zoom.
// Embededd view expects touch add, touch change and touch remove for
// 2 fingers.
TEST_P(EmbeddedViewTouchConformanceTest, Pinch) {
  const int kTapX = 3 * display_width_as_int() / 4;
  const int kTapY = 3 * display_height_as_int() / 4;

  futi::TouchScreenSimulateMultiFingerGestureRequest pinch_req;
  pinch_req.start_locations() = {fuchsia_math::Vec{kTapX - 5, kTapY},
                                 fuchsia_math::Vec{kTapX + 5, kTapY}};
  pinch_req.end_locations() = {fuchsia_math::Vec{kTapX - 20, kTapY},
                               fuchsia_math::Vec{kTapX + 20, kTapY}};
  pinch_req.move_event_count(3);
  pinch_req.finger_count(2);

  FX_LOGS(INFO) << "Injecting pinch from (" << pinch_req.start_locations().value()[0].x() << ", "
                << pinch_req.start_locations().value()[0].y() << ") ("
                << pinch_req.start_locations().value()[1].x() << ", "
                << pinch_req.start_locations().value()[1].y() << ") to ("
                << pinch_req.end_locations().value()[0].x() << ", "
                << pinch_req.end_locations().value()[0].y() << ") ("
                << pinch_req.end_locations().value()[1].x() << ", "
                << pinch_req.end_locations().value()[1].y()
                << ") with move_event_count = " << pinch_req.move_event_count().value();
  fake_touch_screen_->SimulateMultiFingerGesture(pinch_req);

  FX_LOGS(INFO) << "Waiting for touch event listener to receive response";

  // There are 2 * touch down + 4 * touch change + 2 * touch remove, totally 8 events.
  const size_t kExpectedEventCounts = 8;

  RunLoopUntil([&]() {
    return child_puppet_.touch_listener.events_received().size() >= kExpectedEventCounts;
  });

  auto events_received = child_puppet_.touch_listener.cloned_events_received();
  ASSERT_EQ(events_received.size(), kExpectedEventCounts);

  const int kTapXChildView = display_width_as_int() / 4;
  const int kTapYChildView = display_height_as_int() / 4;

  SortTouchInputRequestByTimeAndTimestampAndPointerId(events_received);

  ExpectLocationAndPhase("add [0]", events_received[0], DevicePixelRatio(), kTapXChildView - 5,
                         kTapYChildView, fup::EventPhase::kAdd, 0);
  ExpectLocationAndPhase("add [1]", events_received[1], DevicePixelRatio(), kTapXChildView + 5,
                         kTapYChildView, fup::EventPhase::kAdd, 1);

  ExpectLocationAndPhase("change [0] 1", events_received[2], DevicePixelRatio(),
                         kTapXChildView - 10, kTapYChildView, fup::EventPhase::kChange, 0);
  ExpectLocationAndPhase("change [1] 1", events_received[3], DevicePixelRatio(),
                         kTapXChildView + 10, kTapYChildView, fup::EventPhase::kChange, 1);

  ExpectLocationAndPhase("change [0] 2", events_received[4], DevicePixelRatio(),
                         kTapXChildView - 15, kTapYChildView, fup::EventPhase::kChange, 0);
  ExpectLocationAndPhase("change [1] 2", events_received[5], DevicePixelRatio(),
                         kTapXChildView + 15, kTapYChildView, fup::EventPhase::kChange, 1);

  ExpectLocationAndPhase("remove [0]", events_received[6], DevicePixelRatio(), kTapXChildView - 15,
                         kTapYChildView, fup::EventPhase::kRemove, 0);
  ExpectLocationAndPhase("remove [1]", events_received[7], DevicePixelRatio(), kTapXChildView + 15,
                         kTapYChildView, fup::EventPhase::kRemove, 1);

  // The parent should not have received any pointer events.
  EXPECT_TRUE(parent_puppet_.touch_listener.events_received().empty());
}

}  //  namespace ui_conformance_testing
