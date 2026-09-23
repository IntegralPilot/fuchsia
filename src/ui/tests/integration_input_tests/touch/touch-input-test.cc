// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.fonts/cpp/fidl.h>
#include <fidl/fuchsia.input.injection/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.net.interfaces/cpp/fidl.h>
#include <fidl/fuchsia.posix.socket/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_token_pair.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <utility>
#include <vector>

#include <gtest/gtest.h>
#include <src/lib/timekeeper/clock.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/portable_ui_test.h>

// This test exercises the touch input dispatch path from Input Pipeline to a Scenic client. It
// is a multi-component test, and carefully avoids sleeping or polling for component
// coordination.
// - It runs real Scene Manager and Scenic components.
// - It uses a fake display controller; the physical device is unused.
//
// Components involved
// - This test program
// - Scene Manager
// - Scenic
// - Child view, a Scenic client
//
// Touch dispatch path
// - Test program's injection -> Input Pipeline -> Scenic -> Child view
//
// Setup sequence
// - The test sets up this view hierarchy:
//   - Top level scene, owned by Scene Manager.
//   - Child view, owned by the ui client.
// - The test waits for a Scenic event that verifies the child has UI content in the scene
// graph.
// - The test injects input into Input Pipeline, emulating a display's touch report.
// - Input Pipeline dispatches the touch event to Scenic, which in turn dispatches it to the
// child.
// - The child receives the touch event and reports back to the test over a custom test-only
// FIDL.
// - Test waits for the child to report a touch; when the test receives the report, the test
// quits
//   successfully.
//
// This test uses the realm_builder library to construct the topology of components
// and routes services between them. For v2 components, every test driver component
// sits as a child of test_manager in the topology. Thus, the topology of a test
// driver component such as this one looks like this:
//
//     test_manager
//         |
//   touch-input-test.cml (this component)
//
// With the usage of the realm_builder library, we construct a realm during runtime
// and then extend the topology to look like:
//
//    test_manager
//         |
//   touch-input-test.cml (this component)
//         |
//   <created realm root>
//      /      \
//   scenic  scene_manager
//
// For more information about testing v2 components and realm_builder,
// visit the following links:
//
// Testing: https://fuchsia.dev/fuchsia-src/concepts/testing/v2
// Realm Builder: https://fuchsia.dev/fuchsia-src/development/components/v2/realm_builder

namespace {

// Types imported for the realm_builder library.
using component_testing::ChildRef;
using component_testing::LocalComponentImpl;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Route;

// Alias for Component child name as provided to Realm Builder.
using ChildName = std::string;

// Max timeout in failure cases.
// Set this as low as you can that still works across all test platforms.
constexpr zx::duration kTimeout = zx::min(5);

// Maximum distance between two physical pixel coordinates so that they are considered equal.
constexpr double kEpsilon = 0.5f;

// Number of move events to generate for swipe gestures.
constexpr auto kMoveEventCount = 5;

// The dimensions of the fake display used in tests. Used in calculating the expected distance
// between any two tap events present in the response to a swipe event.
// Note: These values are currently hard coded in the fake display and should be changed
// accordingly.
// TODO(https://fxbug.dev/42062819): Remove the dependency of the tests on these hard coded
// values.
constexpr auto kDisplayWidth = 1280;
constexpr auto kDisplayHeight = 800;

using time_utc = timekeeper::time_utc;

constexpr auto kMockResponseListener = "response_listener";

void ExpectLocationAndPhase(
    const fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest& e, double expected_x,
    double expected_y, fuchsia_ui_pointer::EventPhase expected_phase) {
  auto pixel_scale = e.device_pixel_ratio().has_value() ? e.device_pixel_ratio().value() : 1;
  auto actual_x = pixel_scale * e.local_x().value();
  auto actual_y = pixel_scale * e.local_y().value();
  EXPECT_NEAR(expected_x, actual_x, kEpsilon);
  EXPECT_NEAR(expected_y, actual_y, kEpsilon);
  EXPECT_EQ(expected_phase, e.phase());
}

std::vector<float> ConfigsToTest() {
  std::vector<float> configs;
  // TODO: https://fxbug.dev/42082519 - Test for DPR=2.0, too.
  configs.push_back(2.f);
  return configs;
}

template <typename T>
std::vector<std::tuple<T>> AsTuples(const std::vector<T>& v) {
  std::vector<std::tuple<T>> result;
  for (const auto& elt : v) {
    result.push_back(elt);
  }

  return result;
}

enum class TapLocation { kTopLeft, kTopRight };

enum class SwipeGesture {
  UP = 1,
  DOWN,
  LEFT,
  RIGHT,
};

struct ExpectedSwipeEvent {
  double x = 0, y = 0;
  fuchsia_ui_pointer::EventPhase phase;
};

struct InjectSwipeParams {
  SwipeGesture direction = SwipeGesture::UP;
  int begin_x = 0, begin_y = 0;
  std::vector<ExpectedSwipeEvent> expected_events;
};

// Checks whether all the coordinates in |expected_events| are contained in |actual_events|.
void AssertSwipeEvents(
    const std::vector<fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest>&
        actual_events,
    const std::vector<ExpectedSwipeEvent>& expected_events) {
  FX_DCHECK(actual_events.size() == expected_events.size());

  for (size_t i = 0; i < actual_events.size(); i++) {
    const auto& actual_x = actual_events[i].local_x().value();
    const auto& actual_y = actual_events[i].local_y().value();

    const auto& [expected_x, expected_y, expected_phase] = expected_events[i];

    EXPECT_NEAR(actual_x, expected_x, kEpsilon);
    EXPECT_NEAR(actual_y, expected_y, kEpsilon);
    EXPECT_EQ(actual_events[i].phase(), expected_phase);
  }
}

InjectSwipeParams GetLeftSwipeParams() {
  std::vector<ExpectedSwipeEvent> expected_events;
  auto tap_distance = static_cast<double>(kDisplayWidth) / static_cast<double>(kMoveEventCount);

  // As the child view is rotated by 90 degrees, a swipe in the middle of the display from the
  // right edge to the left edge should appear as a swipe in the middle of the screen from the
  // top edge to the the bottom edge.
  for (double i = 0; i <= static_cast<double>(kMoveEventCount); i++) {
    expected_events.push_back({
        .x = static_cast<double>(kDisplayHeight) / 2,
        .y = i * tap_distance,
        .phase = fuchsia_ui_pointer::EventPhase::kChange,
    });
  }

  expected_events[0].phase = fuchsia_ui_pointer::EventPhase::kAdd;
  expected_events.push_back({
      .x = expected_events.back().x,
      .y = expected_events.back().y,
      .phase = fuchsia_ui_pointer::EventPhase::kRemove,
  });

  return {.direction = SwipeGesture::LEFT,
          .begin_x = kDisplayWidth,
          .begin_y = kDisplayHeight / 2,
          .expected_events = std::move(expected_events)};
}

InjectSwipeParams GetRightSwipeParams() {
  std::vector<ExpectedSwipeEvent> expected_events;
  auto tap_distance = static_cast<double>(kDisplayWidth) / static_cast<double>(kMoveEventCount);

  // As the child view is rotated by 90 degrees, a swipe in the middle of the display from
  // the left edge to the right edge should appear as a swipe in the middle of the screen from
  // the bottom edge to the top edge.
  for (double i = static_cast<double>(kMoveEventCount); i >= 0; i--) {
    expected_events.push_back({
        .x = static_cast<double>(kDisplayHeight) / 2,
        .y = i * tap_distance,
        .phase = fuchsia_ui_pointer::EventPhase::kChange,
    });
  }

  expected_events[0].phase = fuchsia_ui_pointer::EventPhase::kAdd;
  expected_events.push_back({
      .x = expected_events.back().x,
      .y = expected_events.back().y,
      .phase = fuchsia_ui_pointer::EventPhase::kRemove,
  });

  return {.direction = SwipeGesture::RIGHT,
          .begin_x = 0,
          .begin_y = kDisplayHeight / 2,
          .expected_events = std::move(expected_events)};
}

InjectSwipeParams GetUpwardSwipeParams() {
  std::vector<ExpectedSwipeEvent> expected_events;
  auto tap_distance = static_cast<double>(kDisplayHeight) / static_cast<double>(kMoveEventCount);

  // As the child view is rotated by 90 degrees, a swipe in the middle of the display from the
  // bottom edge to the top edge should appear as a swipe in the middle of the screen from the
  // right edge to the left edge.
  for (double i = static_cast<double>(kMoveEventCount); i >= 0; i--) {
    expected_events.push_back({
        .x = i * tap_distance,
        .y = static_cast<double>(kDisplayWidth) / 2,
        .phase = fuchsia_ui_pointer::EventPhase::kChange,
    });
  }

  expected_events[0].phase = fuchsia_ui_pointer::EventPhase::kAdd;
  expected_events.push_back({
      .x = expected_events.back().x,
      .y = expected_events.back().y,
      .phase = fuchsia_ui_pointer::EventPhase::kRemove,
  });

  return {.direction = SwipeGesture::UP,
          .begin_x = kDisplayWidth / 2,
          .begin_y = kDisplayHeight,
          .expected_events = std::move(expected_events)};
}

InjectSwipeParams GetDownwardSwipeParams() {
  std::vector<ExpectedSwipeEvent> expected_events;
  auto tap_distance = static_cast<double>(kDisplayHeight) / static_cast<double>(kMoveEventCount);

  // As the child view is rotated by 90 degrees, a swipe in the middle of the display from the
  // top edge to the bottom edge should appear as a swipe in the middle of the screen from the
  // left edge to the right edge.
  for (double i = 0; i <= static_cast<double>(kMoveEventCount); i++) {
    expected_events.push_back({
        .x = i * tap_distance,
        .y = static_cast<double>(kDisplayWidth) / 2,
        .phase = fuchsia_ui_pointer::EventPhase::kChange,
    });
  }

  expected_events[0].phase = fuchsia_ui_pointer::EventPhase::kAdd;
  expected_events.push_back({
      .x = expected_events.back().x,
      .y = expected_events.back().y,
      .phase = fuchsia_ui_pointer::EventPhase::kRemove,
  });

  return {.direction = SwipeGesture::DOWN,
          .begin_x = kDisplayWidth / 2,
          .begin_y = 0,
          .expected_events = std::move(expected_events)};
}

class ResponseState {
 public:
  const std::vector<fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest>&
  events_received() {
    return events_received_;
  }
  bool ready_to_inject() const { return ready_to_inject_; }

 private:
  friend class ResponseListenerServer;
  std::vector<fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest> events_received_;
  bool ready_to_inject_ = false;
};

// This component implements the test.touch.ResponseListener protocol
// and the interface for a RealmBuilder LocalComponentImpl. A LocalComponentImpl
// is a component that is implemented here in the test, as opposed to elsewhere
// in the system. When it's inserted to the realm, it will act like a proper
// component. This is accomplished, in part, because the realm_builder
// library creates the necessary plumbing. It creates a manifest for the component
// and routes all capabilities to and from it.
class ResponseListenerServer : public fidl::Server<fuchsia_ui_test_input::TouchInputListener>,
                               public fidl::Server<fuchsia_ui_test_input::TestAppStatusListener>,
                               public LocalComponentImpl {
 public:
  explicit ResponseListenerServer(async_dispatcher_t* dispatcher,
                                  std::weak_ptr<ResponseState> state)
      : dispatcher_(dispatcher), state_(std::move(state)) {}

  // |fuchsia_ui_test_input::TouchInputListener|
  void ReportTouchInput(ReportTouchInputRequest& request,
                        ReportTouchInputCompleter::Sync& completer) override {
    if (auto s = state_.lock()) {
      s->events_received_.push_back(std::move(request));
    }
  }

  // |fuchsia_ui_test_input::TestAppStatusListener|
  void ReportStatus(ReportStatusRequest& req, ReportStatusCompleter::Sync& completer) override {
    if (req.status() == fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered) {
      if (auto s = state_.lock()) {
        s->ready_to_inject_ = true;
      }
    }

    completer.Reply();
  }

  // |fuchsia_ui_test_input::TestAppStatusListener|
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_ui_test_input::TestAppStatusListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "TestAppStatusListener Received an unknown method with ordinal "
                     << metadata.method_ordinal;
  }

  // |LocalComponentImpl::Start|
  // When the component framework requests for this component to start, this
  // method will be invoked by the realm_builder library.
  void OnStart() override {
    // When this component starts, add a binding to the test.touch.ResponseListener
    // protocol to this component's outgoing directory.
    outgoing()->AddProtocol<fuchsia_ui_test_input::TouchInputListener>(
        touch_input_listener_bindings_.CreateHandler(this, dispatcher_,
                                                     fidl::kIgnoreBindingClosure));
    outgoing()->AddProtocol<fuchsia_ui_test_input::TestAppStatusListener>(
        app_status_listener_bindings_.CreateHandler(this, dispatcher_,
                                                    fidl::kIgnoreBindingClosure));
  }

 private:
  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TouchInputListener>
      touch_input_listener_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TestAppStatusListener>
      app_status_listener_bindings_;
  std::weak_ptr<ResponseState> state_;
};

template <typename... Ts>
class TouchInputBase : public ui_testing::PortableUITest,
                       public testing::WithParamInterface<std::tuple<float, Ts...>> {
 protected:
  ~TouchInputBase() override {
    FX_CHECK(touch_injection_request_count() > 0) << "injection expected but didn't happen.";
  }

  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  void SetUp() override {
    ui_testing::PortableUITest::SetUp();

    // Post a "just in case" quit task, if the test hangs.
    async::PostDelayedTask(
        dispatcher(),
        [] { FX_LOGS(FATAL) << "\n\n>> Test did not complete in time, terminating.  <<\n\n"; },
        kTimeout);

    // Register input injection device.
    FX_LOGS(INFO) << "Registering input injection device";
    RegisterTouchScreen();

    // Wait until eager client view is rendering to proceed with the test.
    FX_LOGS(INFO) << "Wait for view presentation";
    WaitForViewPresentation();

    FX_LOGS(INFO) << "Wait for test app status: kHandlersRegistered";
    RunLoopUntil([&]() { return response_state()->ready_to_inject(); });
    FX_LOGS(INFO) << "test app status: kHandlersRegistered";
  }

  bool LastEventReceivedMatchesPhase(fuchsia_ui_pointer::EventPhase phase,
                                     const std::string& component_name) {
    const auto& events_received = response_state_->events_received();
    if (events_received.empty()) {
      return false;
    }

    const auto& last_event = events_received.back();
    const auto actual_phase = last_event.phase().value();
    auto actual_component_name = last_event.component_name().value();

    FX_LOGS(INFO) << "Expecting event for component " << component_name << " at phase ("
                  << static_cast<uint32_t>(phase) << ")";
    FX_LOGS(INFO) << "Received event for component " << actual_component_name << " at phaase ("
                  << static_cast<uint32_t>(actual_phase) << ")";

    return phase == actual_phase && actual_component_name == component_name;
  }

  void InjectInput(TapLocation tap_location) {
    // The /config/data/display_rotation (90) specifies how many degrees to rotate the
    // presentation child view, counter-clockwise, in a right-handed coordinate system. Thus,
    // the user observes the child view to rotate *clockwise* by that amount (90).
    //
    // Hence, a tap in the center of the display's top-right quadrant is observed by the child
    // view as a tap in the center of its top-left quadrant.
    auto touch = std::make_unique<fuchsia_ui_input::TouchscreenReport>();
    switch (tap_location) {
      case TapLocation::kTopLeft:
        // center of top right quadrant -> ends up as center of top left quadrant
        InjectTap(/* x = */ 3 * display_width() / 4, /* y = */ display_height() / 4);
        break;
      case TapLocation::kTopRight:
        // center of bottom right quadrant -> ends up as center of top right quadrant
        InjectTap(/* x = */ 3 * display_width() / 4, /* y = */ 3 * display_height() / 4);
        break;
      default:
        FX_NOTREACHED();
    }
  }

  // Inject directly into Input Pipeline, using fuchsia.input.injection FIDLs. A swipe gesture is
  // mimicked by injecting |swipe_length| touch events across the length of the display, with a
  // delay of 10 msec. For the fake display used in this test, and swipe_length=N, this results in
  // events separated by 50 pixels.
  void InjectEdgeToEdgeSwipe(SwipeGesture direction, int begin_x, int begin_y) {
    int x_dir = 0, y_dir = 0;
    switch (direction) {
      case SwipeGesture::UP:
        y_dir = -1;
        break;
      case SwipeGesture::DOWN:
        y_dir = 1;
        break;
      case SwipeGesture::LEFT:
        x_dir = -1;
        break;
      case SwipeGesture::RIGHT:
        x_dir = 1;
        break;
      default:
        FX_NOTREACHED();
    }

    fuchsia_ui_test_input::TouchScreenSimulateSwipeRequest swipe_request;
    swipe_request.start_location() = {begin_x, begin_y};
    swipe_request.end_location() = {static_cast<int32_t>(begin_x + (x_dir * display_width())),
                                    static_cast<int32_t>(begin_y + (y_dir * display_height()))};
    // Generate move events 50 pixels apart.
    swipe_request.move_event_count(kMoveEventCount);

    InjectSwipe(/* start_x = */ begin_x, /* start_y = */ begin_y,
                /* end_x = */ begin_x + (x_dir * display_width()),
                /* end_y = */ begin_y + (y_dir * display_height()),
                /* move_event_count = */ kMoveEventCount);
  }
  uint32_t display_rotation() override { return 90; }

  // TODO: https://fxbug.dev/42082519 - Test for DPR=2.0, too.
  float device_pixel_ratio() override { return 1.f; }

  std::shared_ptr<ResponseState> response_state() const { return response_state_; }

 private:
  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |ResponseListener| service in the constructed realm.
    realm_builder().AddLocalChild(kMockResponseListener, [d = dispatcher(), s = response_state_]() {
      return std::make_unique<ResponseListenerServer>(d, s);
    });
  }

  std::shared_ptr<ResponseState> response_state_ = std::make_shared<ResponseState>();
};

template <typename... Ts>
class CppInputTestBase : public TouchInputBase<Ts...> {
 protected:
  static constexpr auto kCppFlatlandClient = "touch-flatland-client";

 private:
  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {std::make_pair(kCppFlatlandClient, kCppFlatlandClientUrl)};
  }

  std::vector<Route> GetTestRoutes() override {
    return {
        {.capabilities =
             {Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TouchInputListener>},
              Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>}},
         .source = ChildRef{kMockResponseListener},
         .targets = {ChildRef{kCppFlatlandClient}}},
        {.capabilities =
             {Protocol{fidl::DiscoverableProtocolName<fuchsia_element::GraphicalPresenter>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::FlatlandFactory>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Allocator>}},
         .source = ui_testing::PortableUITest::kTestUIStackRef,
         .targets = {ChildRef{kCppFlatlandClient}}},
    };
  }

  static constexpr auto kCppFlatlandClientUrl = "#meta/touch-flatland-client.cm";
};

class CppInputTest : public CppInputTestBase<> {};
INSTANTIATE_TEST_SUITE_P(CppInputTestParametized, CppInputTest,
                         testing::ValuesIn(AsTuples(ConfigsToTest())));

TEST_P(CppInputTest, CppClientTap) {
  InjectInput(TapLocation::kTopLeft);
  RunLoopUntil([this] {
    return LastEventReceivedMatchesPhase(fuchsia_ui_pointer::EventPhase::kRemove,
                                         kCppFlatlandClient);
  });

  const auto& events_received = this->response_state()->events_received();
  ASSERT_EQ(events_received.size(), 2u);
  ExpectLocationAndPhase(events_received[0], static_cast<float>(display_height()) / 4.f,
                         static_cast<float>(display_width()) / 4.f,
                         fuchsia_ui_pointer::EventPhase::kAdd);
  ExpectLocationAndPhase(events_received[1], static_cast<float>(display_height()) / 4.f,
                         static_cast<float>(display_width()) / 4.f,
                         fuchsia_ui_pointer::EventPhase::kRemove);
}

class CppSwipeTest : public CppInputTestBase<InjectSwipeParams> {};
INSTANTIATE_TEST_SUITE_P(
    CppSwipeTestParameterized, CppSwipeTest,
    testing::Combine(testing::ValuesIn(ConfigsToTest()),
                     testing::Values(GetRightSwipeParams(), GetDownwardSwipeParams(),
                                     GetLeftSwipeParams(), GetUpwardSwipeParams())));

TEST_P(CppSwipeTest, CppClientSwipeTest) {
  const auto& [direction, begin_x, begin_y, expected_events] = std::get<1>(GetParam());

  // Inject a swipe on the display. As the child view is rotated by 90 degrees, the direction of
  // the swipe also gets rotated by 90 degrees.
  InjectEdgeToEdgeSwipe(direction, begin_x, begin_y);

  //  Client sends a response for 1 Down, 1 Up and |swipe_length| Move PointerEventPhase events.
  RunLoopUntil([this] {
    FX_LOGS(INFO) << "Events received = " << response_state()->events_received().size();
    FX_LOGS(INFO) << "Events expected = " << kMoveEventCount + 2;
    return response_state()->events_received().size() >= static_cast<uint32_t>(kMoveEventCount + 2);
  });

  const auto& actual_events = response_state()->events_received();
  ASSERT_EQ(actual_events.size(), expected_events.size());
  AssertSwipeEvents(actual_events, expected_events);
}

}  // namespace
