// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.fonts/cpp/fidl.h>
#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.net.interfaces/cpp/fidl.h>
#include <fidl/fuchsia.posix.socket/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.session.scene/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <fidl/fuchsia.web/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <cstddef>
#include <cstdint>
#include <iostream>
#include <memory>
#include <queue>
#include <string>
#include <utility>
#include <vector>

#include <gtest/gtest.h>
#include <src/ui/testing/util/portable_ui_test.h>

namespace {

// Types imported for the realm_builder library.
using component_testing::ChildRef;
using component_testing::Config;
using component_testing::Directory;
using component_testing::LocalComponentImpl;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Route;
using component_testing::VoidRef;

// Alias for Component child name as provided to Realm Builder.
using ChildName = std::string;

// Maximum pointer movement during a clickpad press for the gesture to
// be guaranteed to be interpreted as a click. For movement greater than
// this value, upper layers may, e.g., interpret the gesture as a drag.
//
// This value corresponds to the one used to instantiate the ClickDragHandler
// registered by Input Pipeline in Scene Manager.
constexpr int64_t kClickToDragThreshold = 16.0;

int ButtonsToInt(const std::vector<fuchsia_ui_test_input::MouseButton>& buttons) {
  int result = 0;
  for (const auto& button : buttons) {
    result |= (0x1 >> static_cast<uint32_t>(button));
  }

  return result;
}

// Contains the current mouse input state.
//
// Used to be part of MouseInputListenerServer. The state is now externalized,
// because the new AddLocalChild API does not allow directly inspecting the
// state of the local child component itself.  Instead, this state is shared
// between the test fixture and the local component below.
class MouseInputState {
 public:
  size_t SizeOfEvents() const { return events_.size(); }

  fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest PopEvent() {
    auto e = std::move(events_.front());
    events_.pop();
    return e;
  }

  const fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest& LastEvent() const {
    return events_.back();
  }

  void ClearEvents() { events_ = {}; }

 private:
  friend class MouseInputListenerServer;

  std::queue<fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest> events_;
};

// `MouseInputListener` is a local test protocol that our test apps use to let us know
// what position and button press state the mouse cursor has.
class MouseInputListenerServer : public fidl::Server<fuchsia_ui_test_input::MouseInputListener>,
                                 public fidl::Server<fuchsia_ui_test_input::TestAppStatusListener>,
                                 public LocalComponentImpl {
 public:
  explicit MouseInputListenerServer(async_dispatcher_t* dispatcher,
                                    std::weak_ptr<MouseInputState> mouse_state,
                                    std::weak_ptr<bool> ready_to_inject)
      : dispatcher_(dispatcher),
        mouse_state_(std::move(mouse_state)),
        ready_to_inject_(std::move(ready_to_inject)) {}

  void ReportMouseInput(ReportMouseInputRequest& request,
                        ReportMouseInputCompleter::Sync& completer) override {
    if (auto s = mouse_state_.lock()) {
      s->events_.push(std::move(request));
    }
  }

  void ReportStatus(ReportStatusRequest& req, ReportStatusCompleter::Sync& completer) override {
    if (req.status() == fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered) {
      if (auto ready = ready_to_inject_.lock()) {
        *ready = true;
      }
    }

    completer.Reply();
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_ui_test_input::TestAppStatusListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FX_LOGS(WARNING) << "TestAppStatusListener Received an unknown method with ordinal "
                     << metadata.method_ordinal;
  }

  // When the component framework requests for this component to start, this
  // method will be invoked by the realm_builder library.
  void OnStart() override {
    outgoing()->AddProtocol<fuchsia_ui_test_input::MouseInputListener>(
        mouse_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    outgoing()->AddProtocol<fuchsia_ui_test_input::TestAppStatusListener>(
        app_status_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  }

 private:
  // Not owned.
  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::MouseInputListener> mouse_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TestAppStatusListener> app_status_bindings_;
  std::weak_ptr<MouseInputState> mouse_state_;
  std::weak_ptr<bool> ready_to_inject_;
};

constexpr auto kMouseInputListener = "mouse_input_listener";

struct Position {
  double x = 0.0;
  double y = 0.0;
};

class MouseInputBase : public ui_testing::PortableUITest {
 protected:
  MouseInputBase() : mouse_state_(std::make_shared<MouseInputState>()) {}

  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  void SetUp() override {
    ui_testing::PortableUITest::SetUp();

    // Register fake mouse device.
    RegisterMouse();
  }

  void TearDown() override {
    ui_testing::PortableUITest::TearDown();

    // at the end of test, ensure event queue is empty.
    ASSERT_EQ(mouse_state_->SizeOfEvents(), 0u);
  }

  // Helper method for checking the test.mouse.MouseInputListener response from the client app.
  static void VerifyEvent(
      fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest& pointer_data,
      double expected_x, double expected_y,
      const std::vector<fuchsia_ui_test_input::MouseButton>& expected_buttons,
      const fuchsia_ui_test_input::MouseEventPhase expected_phase,
      const std::string& component_name) {
    FX_LOGS(INFO) << "Client received mouse change at (" << pointer_data.local_x().value() << ", "
                  << pointer_data.local_y().value() << ") with buttons "
                  << ButtonsToInt(pointer_data.buttons().value()) << ".";
    FX_LOGS(INFO) << "Expected mouse change is at approximately (" << expected_x << ", "
                  << expected_y << ") with buttons " << ButtonsToInt(expected_buttons) << ".";

    // Allow for minor rounding differences in coordinates.
    // Note: These approximations don't account for `PointerMotionDisplayScaleHandler`
    // or `PointerMotionSensorScaleHandler`. We will need to do so in order to validate
    // larger motion or different sized displays.
    EXPECT_NEAR(pointer_data.local_x().value(), expected_x, 1);
    EXPECT_NEAR(pointer_data.local_y().value(), expected_y, 1);
    EXPECT_EQ(pointer_data.buttons().value(), expected_buttons);
    EXPECT_EQ(pointer_data.phase().value(), expected_phase);
    EXPECT_EQ(pointer_data.component_name().value(), component_name);
  }

  static void VerifyEventLocationOnTheRightOfExpectation(
      fuchsia_ui_test_input::MouseInputListenerReportMouseInputRequest& pointer_data,
      double expected_x_min, double expected_y,
      const std::vector<fuchsia_ui_test_input::MouseButton>& expected_buttons,
      const fuchsia_ui_test_input::MouseEventPhase expected_phase,
      const std::string& component_name) {
    FX_LOGS(INFO) << "Client received mouse change at (" << pointer_data.local_x().value() << ", "
                  << pointer_data.local_y().value() << ") with buttons "
                  << ButtonsToInt(pointer_data.buttons().value()) << ".";
    FX_LOGS(INFO) << "Expected mouse change is at approximately (>" << expected_x_min << ", "
                  << expected_y << ") with buttons " << ButtonsToInt(expected_buttons) << ".";

    EXPECT_GT(pointer_data.local_x().value(), expected_x_min);
    EXPECT_NEAR(pointer_data.local_y().value(), expected_y, 1);
    EXPECT_EQ(pointer_data.buttons().value(), expected_buttons);
    EXPECT_EQ(pointer_data.phase().value(), expected_phase);
    EXPECT_EQ(pointer_data.component_name().value(), component_name);
  }

  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |MouseInputListener| service in the constructed realm.
    auto* d = dispatcher();
    realm_builder().AddLocalChild(
        kMouseInputListener, [d, mouse_state = mouse_state_, ready_to_inject = ready_to_inject_]() {
          return std::make_unique<MouseInputListenerServer>(d, mouse_state, ready_to_inject);
        });
  }

  std::shared_ptr<MouseInputState> mouse_state_;
  std::shared_ptr<bool> ready_to_inject_ = std::make_shared<bool>(false);

  // Use a DPR other than 1.0, so that logical and physical coordinate spaces
  // are different.
  float device_pixel_ratio() override { return 2.f; }

  static constexpr auto kFontsProvider = "fonts_provider";
  static constexpr auto kFontsProviderUrl = "#meta/font_provider_hermetic_for_test.cm";
};

class ChromiumInputTest : public MouseInputBase {
 public:
  void SetUp() override {
    MouseInputBase::SetUp();
    WaitForViewPresentation();

    FX_LOGS(INFO) << "Wait for Chromium send out ready";
    RunLoopUntil([this]() { return *(ready_to_inject_); });
  }

 protected:
  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {
        std::make_pair(kMouseInputChromium, kMouseInputChromiumUrl),
    };
  }

  std::vector<std::pair<ChildName, std::string>> GetTestComponents() override {
    return {
        std::make_pair(kBuildInfoProvider, kBuildInfoProviderUrl),
        std::make_pair(kFontsProvider, kFontsProviderUrl),
        std::make_pair(kMemoryPressureSignaler, kMemoryPressureSignalerUrl),
        std::make_pair(kNetstack, kNetstackUrl),
        std::make_pair(kFakeCobalt, kFakeCobaltUrl),
        std::make_pair(kWebContextProvider, kWebContextProviderUrl),
    };
  }

  std::vector<Route> GetTestRoutes() override {
    return GetChromiumRoutes(ChildRef{kMouseInputChromium});
  }

  // Routes needed to setup Chromium client.
  static std::vector<Route> GetChromiumRoutes(ChildRef target) {
    return {
        {.capabilities =
             {
                 Protocol{fidl::DiscoverableProtocolName<
                     fuchsia_accessibility_semantics::SemanticsManager>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_element::GraphicalPresenter>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Allocator>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Flatland>},
             },
         .source = kTestUIStackRef,
         .targets = {target}},
        {.capabilities =
             {
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_kernel::VmexResource>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_process::Launcher>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_vulkan_loader::Loader>},
             },
         .source = ParentRef(),
         .targets = {target}},
        {
            .capabilities =
                {
                    Protocol{fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>},
                },
            .source = ParentRef(),
            .targets =
                {
                    target,
                    ChildRef{kFontsProvider},
                    ChildRef{kMemoryPressureSignaler},
                    ChildRef{kBuildInfoProvider},
                    ChildRef{kWebContextProvider},
                    ChildRef{kFakeCobalt},
                    // Not including kNetstack here, since it emits spurious
                    // FATAL errors.
                },
        },
        {.capabilities =
             {
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::MouseInputListener>},
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>},
             },
         .source = ChildRef{kMouseInputListener},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_fonts::Provider>}},
         .source = ChildRef{kFontsProvider},
         .targets = {target}},
        {.capabilities =
             {
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>},
             },
         .source = ParentRef(),
         .targets = {target, ChildRef{kFontsProvider}}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_memorypressure::Provider>}},
         .source = ChildRef{kMemoryPressureSignaler},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_net_interfaces::State>}},
         .source = ChildRef{kNetstack},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_web::ContextProvider>}},
         .source = ChildRef{kWebContextProvider},
         .targets = {target}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_metrics::MetricEventLoggerFactory>}},
         .source = ChildRef{kFakeCobalt},
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem::Allocator>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem2::Allocator>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}, target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_scheduler::RoleManager>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_kernel::RootJobForInspect>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_kernel::Stats>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_posix_socket::Provider>}},
         .source = ChildRef{kNetstack},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_buildinfo::Provider>}},
         .source = ChildRef{kBuildInfoProvider},
         .targets = {ChildRef{kWebContextProvider}, target}},
        {.capabilities =
             {
                 Directory{
                     .name = "root-ssl-certificates",
                     .type = fidl::NaturalToHLCPP(fuchsia_component_decl::DependencyType::kStrong),
                 },
                 Directory{
                     .name = "tzdata-icu",
                     .type = fidl::NaturalToHLCPP(fuchsia_component_decl::DependencyType::kStrong),
                 },
             },
         .source = ParentRef(),
         .targets = {ChildRef{kWebContextProvider}}},
    };
  }

  // TODO(https://fxbug.dev/42136267): EnsureMouseIsReadyAndGetPosition will send a mouse click
  // (down and up) and wait for response to ensure the mouse is ready to use. We will retry a
  // mouse click if we can not get the mouseup response in small timeout. This function returns
  // the cursor position in WebEngine coordinate system.
  Position EnsureMouseIsReadyAndGetPosition() {
    for (int retry = 0; retry < kMaxRetry; retry++) {
      // Mouse down and up.
      SimulateMouseEvent(/* pressed_buttons = */ {fuchsia_ui_test_input::MouseButton::kFirst},
                         /* movement_x = */ 0, /* movement_y = */ 0);
      SimulateMouseEvent(/* pressed_buttons = */ {}, /* movement_x = */ 0, /* movement_y = */ 0);

      auto wait_until_last_event_phase_or_timeout =
          [this](fuchsia_ui_test_input::MouseEventPhase event_phase) {
            return RunLoopWithTimeoutOrUntil(
                [this, event_phase] {
                  return mouse_state_->SizeOfEvents() > 0 &&
                         mouse_state_->LastEvent().phase() == event_phase;
                },
                kFirstEventRetryInterval);
          };

      bool got_mouse_up =
          wait_until_last_event_phase_or_timeout(fuchsia_ui_test_input::MouseEventPhase::kUp);

      if (got_mouse_up) {
        // There is an issue we found in retry that the mouse up we got may
        // come from previous retry loop, then EnsureMouseIsReadyAndGetPosition
        // exit and the down-up event caught by test body, and break the test
        // expectation. Here we inject 1 more wheel event, because wheel event
        // only injected once, wheel we receive the wheel event, we know all
        // events from EnsureMouseIsReadyAndGetPosition are processed.
        SimulateMouseScroll(/* pressed_buttons = */ {}, /* scroll_x = */ 0, /* scroll_y = */ 1);
        wait_until_last_event_phase_or_timeout(fuchsia_ui_test_input::MouseEventPhase::kWheel);
        Position p;
        p.x = mouse_state_->LastEvent().local_x().value();
        p.y = mouse_state_->LastEvent().local_y().value();
        mouse_state_->ClearEvents();
        return p;
      }
    }

    FX_LOGS(FATAL) << "Can not get mouse click in max retries " << kMaxRetry;
    return Position{};
  }

  static constexpr auto kMouseInputChromium = "mouse-input-chromium";
  static constexpr auto kMouseInputChromiumUrl = "#meta/mouse-input-chromium.cm";

  static constexpr auto kWebContextProvider = "web_context_provider";
  static constexpr auto kWebContextProviderUrl =
      "fuchsia-pkg://fuchsia.com/web_engine#meta/context_provider.cm";

  static constexpr auto kMemoryPressureSignaler = "memory_pressure_signaler";
  static constexpr auto kMemoryPressureSignalerUrl = "#meta/memory_pressure_signaler.cm";

  static constexpr auto kNetstack = "netstack";
  static constexpr auto kNetstackUrl = "#meta/netstack.cm";

  static constexpr auto kBuildInfoProvider = "build_info_provider";
  static constexpr auto kBuildInfoProviderUrl = "#meta/fake_build_info.cm";

  static constexpr auto kFakeCobalt = "cobalt";
  static constexpr auto kFakeCobaltUrl = "#meta/fake_cobalt.cm";

  // The first event to WebEngine may lost, see EnsureMouseIsReadyAndGetPosition. Retry to ensure
  // WebEngine is ready to process events.
  static constexpr auto kFirstEventRetryInterval = zx::sec(1);

  // To avoid retry to timeout, limit 10 retries, if still not ready, fail it with meaningful
  // error.
  static const int kMaxRetry = 10;
};

TEST_F(ChromiumInputTest, ChromiumMouseMove) {
  auto initial_position = EnsureMouseIsReadyAndGetPosition();

  double initial_x = initial_position.x;
  double initial_y = initial_position.y;

  SimulateMouseEvent(/* pressed_buttons = */ {},
                     /* movement_x = */ 5, /* movement_y = */ 0);
  RunLoopUntil([this] { return mouse_state_->SizeOfEvents() == 1; });

  auto event_move = mouse_state_->PopEvent();

  VerifyEventLocationOnTheRightOfExpectation(
      event_move,
      /*expected_x_min=*/initial_x,
      /*expected_y=*/initial_y,
      /*expected_buttons=*/{},
      /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kMove,
      /*component_name=*/"mouse-input-chromium");
}

TEST_F(ChromiumInputTest, DISABLED_ChromiumMouseDownMoveUp) {
  auto initial_position = EnsureMouseIsReadyAndGetPosition();

  double initial_x = initial_position.x;
  double initial_y = initial_position.y;

  SimulateMouseEvent(/* pressed_buttons = */ {fuchsia_ui_test_input::MouseButton::kFirst},
                     /* movement_x = */ 0, /* movement_y = */ 0);
  SimulateMouseEvent(/* pressed_buttons = */ {fuchsia_ui_test_input::MouseButton::kFirst},
                     /* movement_x = */ kClickToDragThreshold, /* movement_y = */ 0);
  SimulateMouseEvent(/* pressed_buttons = */ {}, /* movement_x = */ 0, /* movement_y = */ 0);

  RunLoopUntil([this] { return mouse_state_->SizeOfEvents() == 3; });

  auto event_down = mouse_state_->PopEvent();
  auto event_move = mouse_state_->PopEvent();
  auto event_up = mouse_state_->PopEvent();

  VerifyEvent(event_down,
              /*expected_x=*/initial_x,
              /*expected_y=*/initial_y,
              /*expected_buttons=*/{fuchsia_ui_test_input::MouseButton::kFirst},
              /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kDown,
              /*component_name=*/"mouse-input-chromium");
  VerifyEventLocationOnTheRightOfExpectation(
      event_move,
      /*expected_x_min=*/initial_x,
      /*expected_y=*/initial_y,
      /*expected_buttons=*/{fuchsia_ui_test_input::MouseButton::kFirst},
      /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kMove,
      /*component_name=*/"mouse-input-chromium");
  VerifyEvent(event_up,
              /*expected_x=*/event_move.local_x().value(),
              /*expected_y=*/initial_y,
              /*expected_buttons=*/{},
              /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kUp,
              /*component_name=*/"mouse-input-chromium");
}

TEST_F(ChromiumInputTest, DISABLED_ChromiumMouseWheel) {
  auto initial_position = EnsureMouseIsReadyAndGetPosition();

  double initial_x = initial_position.x;
  double initial_y = initial_position.y;

  SimulateMouseScroll(/* pressed_buttons = */ {}, /* scroll_x = */ 1, /* scroll_y = */ 0);
  RunLoopUntil([this] { return mouse_state_->SizeOfEvents() == 1; });

  auto event_wheel_h = mouse_state_->PopEvent();

  VerifyEvent(event_wheel_h,
              /*expected_x=*/initial_x,
              /*expected_y=*/initial_y,
              /*expected_buttons=*/{},
              /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kWheel,
              /*component_name=*/"mouse-input-chromium");
  // Chromium will scale the count of ticks to pixel.
  // Positive delta in Fuchsia  means scroll left, and scroll left in JS is negative delta.
  EXPECT_LT(event_wheel_h.wheel_x_physical_pixel(), 0);
  EXPECT_EQ(event_wheel_h.wheel_y_physical_pixel(), 0);

  SimulateMouseScroll(/* pressed_buttons = */ {}, /* scroll_x = */ 0, /* scroll_y = */ 1);
  RunLoopUntil([this] { return mouse_state_->SizeOfEvents() == 1; });

  auto event_wheel_v = mouse_state_->PopEvent();

  VerifyEvent(event_wheel_v,
              /*expected_x=*/initial_x,
              /*expected_y=*/initial_y,
              /*expected_buttons=*/{},
              /*expected_phase=*/fuchsia_ui_test_input::MouseEventPhase::kWheel,
              /*component_name=*/"mouse-input-chromium");
  // Chromium will scale the count of ticks to pixel.
  // Positive delta in Fuchsia means scroll up, and scroll up in JS is negative delta.
  EXPECT_LT(event_wheel_v.wheel_y_physical_pixel(), 0);
  EXPECT_EQ(event_wheel_v.wheel_x_physical_pixel(), 0);
}

}  // namespace
