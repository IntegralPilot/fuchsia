// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.input.injection/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/test.accessibility/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <cstddef>
#include <cstdint>
#include <iostream>
#include <memory>
#include <utility>

#include <gtest/gtest.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/portable_ui_test.h>

// This test exercises the pointer injector code in the context of Input Pipeline and a real Scenic
// client. It is a multi-component test, and carefully avoids sleeping or polling for component
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
// - The test waits for a Scenic event that verifies the child has UI content in the scene graph.
// - The test injects input into Input Pipeline, emulating a display's touch report.
// - Input Pipeline dispatches the touch event to Scenic, which in turn dispatches it to the child.
// - The child receives the touch event and reports back to the test over a custom test-only FIDL.
// - Test waits for the child to report a touch; when the test receives the report, the test quits
//   successfully.
//
// This test uses the realm_builder library to construct the topology of components
// and routes services between them. For v2 components, every test driver component
// sits as a child of test_manager in the topology. Thus, the topology of a test
// driver component such as this one looks like this:
//
//     test_manager
//         |
//   pointerinjector-config-test.cml (this component)
//
// With the usage of the realm_builder library, we construct a realm during runtime
// and then extend the topology to look like:
//
//    test_manager
//         |
//   pointerinjector-config-test.cml (this component)
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

// Alias for Component child name as provided to Realm Builder.
using ChildName = std::string;

// Alias for Component Legacy URL as provided to Realm Builder.
using LegacyUrl = std::string;

// Max timeout in failure cases.
// Set this as low as you can that still works across all test platforms.
constexpr zx::duration kTimeout = zx::min(5);

// Maximum distance between two view coordinates so that they are considered equal.
constexpr auto kViewCoordinateEpsilon = 0.01;

constexpr auto kMockResponseListener = "response_listener";

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

// This component implements fuchsia.ui.test.input.TouchInputListener
// and the interface for a RealmBuilder LocalComponent. A LocalComponent
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
    FX_LOGS(INFO) << "ReportTouchInput";
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

struct PointerInjectorConfigTestData {
  int display_rotation;

  float clip_scale = 1.f;
  float clip_translation_x = 0.f;
  float clip_translation_y = 0.f;

  // expected location of the pointer event, in client view space, where the
  // range of the X and Y axes is [0, 1]
  float expected_x;
  float expected_y;
};

void ExpectLocationAndPhase(
    const fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest& e, double expected_x,
    double expected_y, fuchsia_ui_pointer::EventPhase expected_phase) {
  auto pixel_scale = e.device_pixel_ratio().has_value() ? e.device_pixel_ratio().value() : 1;
  auto actual_x = pixel_scale * e.local_x().value();
  auto actual_y = pixel_scale * e.local_y().value();
  EXPECT_NEAR(expected_x, actual_x, kViewCoordinateEpsilon);
  EXPECT_NEAR(expected_y, actual_y, kViewCoordinateEpsilon);
  EXPECT_EQ(expected_phase, e.phase());
}

class PointerInjectorConfigTest
    : public ui_testing::PortableUITest,
      public testing::WithParamInterface<PointerInjectorConfigTestData> {
 protected:
  PointerInjectorConfigTest() = default;
  ~PointerInjectorConfigTest() override {
    FX_CHECK(touch_injection_request_count() > 0) << "injection expected but didn't happen.";
  }

  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  uint32_t display_rotation() override { return GetParam().display_rotation; }

  void SetUp() override {
    ui_testing::PortableUITest::SetUp();
    // Post a "just in case" quit task, if the test hangs.
    async::PostDelayedTask(
        dispatcher(),
        [] { FX_LOGS(FATAL) << "\n\n>> Test did not complete in time, terminating.  <<\n\n"; },
        kTimeout);

    // Get the display dimensions.
    FX_LOGS(INFO) << "Waiting for scenic display info";

    FX_LOGS(INFO) << "Got display_width = " << display_width()
                  << " and display_height = " << display_height();

    // Register input injection device.
    FX_LOGS(INFO) << "Registering input injection device";
    RegisterTouchScreen();

    // Wait until eager client view is rendering to proceed with the test.
    WaitForViewPresentation();

    auto magnifier_connect = realm_root()->component().Connect<test_accessibility::Magnifier>();
    ZX_ASSERT_OK(magnifier_connect);
    fake_magnifier_ = fidl::SyncClient(std::move(magnifier_connect.value()));

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
    FX_LOGS(INFO) << "Received event for component " << actual_component_name << " at phase ("
                  << static_cast<uint32_t>(actual_phase) << ")";

    return phase == actual_phase && actual_component_name == component_name;
  }

  void InjectTapOnTopLeft() {
    int32_t x = 0;
    int32_t y = 0;

    auto test_data = GetParam();

    switch (test_data.display_rotation) {
      case 0:
        x = display_width() / 4;
        y = display_height() / 4;
        break;
      case 90:
        // The /config/data/display_rotation (90) specifies how many degrees to rotate the
        // presentation child view, counter-clockwise, in a right-handed coordinate system. Thus,
        // the user observes the child view to rotate *clockwise* by that amount (90).
        x = 3 * display_width() / 4;
        y = display_height() / 4;
        break;
      default:
        FX_NOTREACHED();
    }

    InjectTap(x, y);
  }

  void SetClipSpaceTransform(float scale, float x, float y, int display_rotation) {
    // HACK HACK HACK
    // TODO(https://fxbug.dev/42081619): Remove this when we move to the new gesture
    // disambiguation protocols.
    //
    // Because the FlatlandAcessibilityView::SetMagnificationTransform
    // hardcodes translation values at a display rotation of 270, this test
    // must normalize the values given the non-270 display rotations
    // config values.
    switch (display_rotation) {
      case 0:
        // Since a display rotation of 270 uses (x, y) as (y, -x), pass
        // in (y, -x), which will yield (x, y) to get an effective display
        // rotation of 0.
        ZX_ASSERT_OK(fake_magnifier_->SetMagnification({scale, y, -x}));
        break;
      case 90:
        // Since a display rotation of 270 uses (x, y) as (y, -x), pass
        // in (-x, -y), which will yield (-y, x) to get an effective display
        // rotation of 90.
        ZX_ASSERT_OK(fake_magnifier_->SetMagnification({scale, -x, -y}));
        break;
      default:
        FX_NOTREACHED();
    }
  }

  float display_width_float() { return static_cast<float>(display_width()); }
  float display_height_float() { return static_cast<float>(display_height()); }

  std::shared_ptr<ResponseState> response_state() { return response_state_; }

  static constexpr auto kCppFlatlandClient = "touch-flatland-client";

 private:
  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {std::make_pair(kCppFlatlandClient, kCppFlatlandClientUrl)};
  }

  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |ResponseListener| service in the constructed realm.
    realm_builder().AddLocalChild(kMockResponseListener,
                                  [d = dispatcher(), s = response_state()]() {
                                    return std::make_unique<ResponseListenerServer>(d, s);
                                  });

    realm_builder().AddRoute(
        {.capabilities =
             {
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TouchInputListener>},
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>},
             },
         .source = ChildRef{kMockResponseListener},
         .targets = {ChildRef{kCppFlatlandClient}}});
    realm_builder().AddRoute(
        {.capabilities =
             {Protocol{fidl::DiscoverableProtocolName<fuchsia_element::GraphicalPresenter>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Allocator>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::FlatlandFactory>}},
         .source = ui_testing::PortableUITest::kTestUIStackRef,
         .targets = {ChildRef{kCppFlatlandClient}}});
  }

  std::shared_ptr<ResponseState> response_state_ = std::make_shared<ResponseState>();

  fidl::SyncClient<test_accessibility::Magnifier> fake_magnifier_;

  static constexpr auto kCppFlatlandClientUrl = "#meta/touch-flatland-client.cm";
};

// Declare test data.
// In all these tests, we tap the center of the top left quadrant of the
// physical display (after rotation), and verify that the client view gets a
// pointer event with the expected coordinates.

// No changes to display rotation or clip space
constexpr PointerInjectorConfigTestData kTestDataBaseCase = {
    .display_rotation = 0, .expected_x = 1.f / 4.f, .expected_y = 1.f / 4.f};

// Test scale by a factor of 2.
// Intuitive argument for these expected coordinates:
// Here we've zoomed into the center of the client view, scaling it up by 2x. So, the touch point
// will have 'migrated' halfway towards the center of the client view:  3/8 instead of 1/4.
constexpr PointerInjectorConfigTestData kTestDataScale = {
    .display_rotation = 0, .clip_scale = 2.f, .expected_x = 3.f / 8.f, .expected_y = 3.f / 8.f};

// Test display rotation by 90 degrees.
// In this case, rotation shouldn't affect what the client view sees.
constexpr PointerInjectorConfigTestData kTestDataRotateAndScale = {
    .display_rotation = 90, .clip_scale = 2.f, .expected_x = 3.f / 8.f, .expected_y = 3.f / 8.f};

// Test scaling and translation.
constexpr float kScale = 3.f;
constexpr float kTranslationX = -0.2f;
constexpr float kTranslationY = 0.1f;
constexpr PointerInjectorConfigTestData kTestDataScaleAndTranslate = {
    .display_rotation = 0,
    .clip_scale = kScale,
    .clip_translation_x = kTranslationX,
    .clip_translation_y = kTranslationY,
    // Terms: 'Original position' + 'movement due to scale' + 'movement due to translation'
    .expected_x = 0.25f + (0.25f * (1.f - 1.f / kScale)) - (kTranslationX / 2.f / kScale),
    .expected_y = 0.25f + (0.25f * (1.f - 1.f / kScale)) - (kTranslationY / 2.f / kScale)};

// Test scaling, translation, and rotation at once.
//
// Here, the translation does affect what the client view sees, so we have to account for it.
// This is what the translation looks like in client view coordinates, where it's rotated 90
// degrees.
constexpr float kClientViewTranslationX = kTranslationY;
constexpr float kClientViewTranslationY = -kTranslationX;
constexpr PointerInjectorConfigTestData kTestDataScaleTranslateRotate = {
    .display_rotation = 90,
    .clip_scale = kScale,
    .clip_translation_x = kTranslationX,
    .clip_translation_y = kTranslationY,
    // Same formula as before, but with different transform values.
    .expected_x = 0.25f + (0.25f * (1.f - 1.f / kScale)) - (kClientViewTranslationX / 2.f / kScale),
    .expected_y =
        0.25f + (0.25f * (1.f - 1.f / kScale)) - (kClientViewTranslationY / 2.f / kScale)};

INSTANTIATE_TEST_SUITE_P(PointerInjectorConfigTestWithParams, PointerInjectorConfigTest,
                         ::testing::Values(kTestDataBaseCase, kTestDataScale,
                                           kTestDataRotateAndScale, kTestDataScaleAndTranslate,
                                           kTestDataScaleTranslateRotate));

TEST_P(PointerInjectorConfigTest, CppClientTapTest) {
  auto test_data = GetParam();

  FX_LOGS(INFO) << "Starting test with params: display_rotation=" << test_data.display_rotation
                << ", clip_scale=" << test_data.clip_scale
                << ", clip_translation_x=" << test_data.clip_translation_x
                << ", clip_translation_y=" << test_data.clip_translation_y
                << ", expected_x=" << test_data.expected_x
                << ", expected_y=" << test_data.expected_y;

  SetClipSpaceTransform(test_data.clip_scale, test_data.clip_translation_x,
                        test_data.clip_translation_y, test_data.display_rotation);

  InjectTapOnTopLeft();

  RunLoopUntil([this] {
    return LastEventReceivedMatchesPhase(fuchsia_ui_pointer::EventPhase::kRemove,
                                         kCppFlatlandClient);
  });

  const auto& events_received = this->response_state()->events_received();
  ASSERT_EQ(events_received.size(), 2u);

  float expect_x = display_width_float() * test_data.expected_x;
  float expect_y = display_height_float() * test_data.expected_y;
  if (test_data.display_rotation == 90) {
    expect_x = display_height_float() * test_data.expected_x;
    expect_y = display_width_float() * test_data.expected_y;
  }

  ExpectLocationAndPhase(events_received[0], expect_x, expect_y,
                         fuchsia_ui_pointer::EventPhase::kAdd);
  ExpectLocationAndPhase(events_received[1], expect_x, expect_y,
                         fuchsia_ui_pointer::EventPhase::kRemove);
}

}  // namespace
