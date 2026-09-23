// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.fonts/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.net.interfaces/cpp/fidl.h>
#include <fidl/fuchsia.posix.socket/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.app/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <fidl/fuchsia.web/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/sys/cpp/component_context.h>
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
#include <vector>

#include <gtest/gtest.h>
#include <src/lib/timekeeper/clock.h>
#include <src/ui/testing/util/portable_ui_test.h>

// This test exercises the touch input dispatch path from Input Pipeline to a Chromium. It is a
// multi-component test, and carefully avoids sleeping or polling for component coordination.
// - It runs real Scene Manager and Scenic components.
// - It uses a fake display controller; the physical device is unused.
//
// Components involved
// - This test program
// - Scene Manager
// - Scenic
// - Chromium
//
// Touch dispatch path
// - Test program's injection -> Input Pipeline -> Scenic -> Chromium
//
// Setup sequence
// - The test sets up this view hierarchy:
//   - Top level scene, owned by Scene Manager.
//   - Chromium
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
//   web-touch-input-test.cml (this component)
//
// With the usage of the realm_builder library, we construct a realm during runtime
// and then extend the topology to look like:
//
//    test_manager
//         |
//   web-touch-input-test.cml (this component)
//         |
//   <created realm root>
//      /      \
//   scenic  input-pipeline
//
// For more information about testing v2 components and realm_builder,
// visit the following links:
//
// Testing: https://fuchsia.dev/fuchsia-src/concepts/testing/v2
// Realm Builder: https://fuchsia.dev/fuchsia-src/development/components/v2/realm_builder

namespace {

// Types imported for the realm_builder library.
using component_testing::ChildRef;
using component_testing::Config;
using component_testing::Dictionary;
using component_testing::Directory;
using component_testing::LocalComponentImpl;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Route;
using component_testing::VoidRef;

// Alias for Component child name as provided to Realm Builder.
using ChildName = std::string;

// Max timeout in failure cases.
// Set this as low as you can that still works across all test platforms.
constexpr zx::duration kTimeout = zx::min(5);

using time_utc = timekeeper::time_utc;

// Maximum distance between two physical pixel coordinates so that they are considered equal.
constexpr double kEpsilon = 0.5f;

constexpr auto kMockResponseListener = "response_listener";

std::vector<float> ConfigsToTest() {
  std::vector<float> configs;
  // TODO: https://fxbug.dev/42082519 - Test for DPR=2.0, too.
  configs.push_back(2.f);
  return configs;
}

template <typename T>
std::vector<std::tuple<T>> AsTuples(std::vector<T> v) {
  std::vector<std::tuple<T>> result;
  for (const auto& elt : v) {
    result.push_back(elt);
  }

  return result;
}

enum class TapLocation { kTopLeft, kTopRight };

class ResponseState {
 public:
  const std::vector<fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest>&
  events_received() {
    return events_received_;
  }

  void ClearEvents() { events_received_.clear(); }

 private:
  friend class ResponseListenerServer;
  std::vector<fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest> events_received_;
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
                                  std::weak_ptr<ResponseState> state,
                                  std::weak_ptr<bool> ready_to_inject)
      : dispatcher_(dispatcher),
        state_(std::move(state)),
        ready_to_inject_(std::move(ready_to_inject)) {}

  // |fuchsia_ui_test_input::TouchInputListener|
  void ReportTouchInput(ReportTouchInputRequest& request,
                        ReportTouchInputCompleter::Sync& completer) override {
    if (auto s = state_.lock()) {
      s->events_received_.push_back(std::move(request));
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

  // |LocalComponentImpl::Start|
  // When the component framework requests for this component to start, this
  // method will be invoked by the realm_builder library.
  void OnStart() override {
    // When this component starts, add a binding to the test.touch.ResponseListener
    // protocol to this component's outgoing directory.
    outgoing()->AddProtocol<fuchsia_ui_test_input::TouchInputListener>(
        touch_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    outgoing()->AddProtocol<fuchsia_ui_test_input::TestAppStatusListener>(
        app_status_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  }

 private:
  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TouchInputListener> touch_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TestAppStatusListener> app_status_bindings_;
  std::weak_ptr<ResponseState> state_;
  std::weak_ptr<bool> ready_to_inject_;
};

class WebEngineTest : public ui_testing::PortableUITest,
                      public testing::WithParamInterface<std::tuple<float>> {
 protected:
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

    WaitForViewPresentation();

    FX_LOGS(INFO) << "Wait for Chromium send out ready";
    RunLoopUntil([this]() { return *(ready_to_inject_); });

    // TODO(b/477639582): Repeating the tap may not be necessary, but adding
    // it for now to stable the test after chromium 145.0.7622.0 roll.
    const int kMaxRetries = 10;
    FX_LOGS(INFO) << "Waiting on app to be ready for input";
    for (int i = 0; i < kMaxRetries; i++) {
      FX_LOGS(INFO) << "Tapping: " << i + 1 << "/" << kMaxRetries;
      // Not quite exactly the location of the text area under test, but since the
      // text area occupies all the screen, it's very likely within the text area.
      InjectTap(display_width() / 2, display_height() / 2);

      RunLoopWithTimeoutOrUntil([&] { return response_state()->events_received().size() >= 1; },
                                zx::sec(1));
      if (response_state()->events_received().size() >= 1) {
        FX_LOGS(INFO) << "App is ready for input events: tapped " << i + 1 << " times";
        break;
      }
    }
    // After 10 taps, the app should be ready for key events.
    ASSERT_TRUE(response_state()->events_received().size() >= 1);
    response_state()->ClearEvents();
  }

  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {
        std::make_pair(kOneChromiumClient, kOneChromiumUrl),
    };
  }

  std::vector<Route> GetTestRoutes() override {
    return GetWebEngineRoutes(ChildRef{kOneChromiumClient});
  }

  std::vector<std::pair<ChildName, std::string>> GetTestComponents() override {
    return {
        std::make_pair(kBuildInfoProvider, kBuildInfoProviderUrl),
        std::make_pair(kFontsProvider, kFontsProviderUrl),
        std::make_pair(kIntl, kIntlUrl),
        std::make_pair(kMemoryPressureSignaler, kMemoryPressureSignalerUrl),
        std::make_pair(kFakeCobalt, kFakeCobaltUrl),
        std::make_pair(kNetstack, kNetstackUrl),
        std::make_pair(kTextManager, kTextManagerUrl),
        std::make_pair(kWebContextProvider, kWebContextProviderUrl),
    };
  }

  void ExpectLocation(const fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest& e,
                      float expected_x, float expected_y, const std::string& component_name) {
    auto pixel_scale = e.device_pixel_ratio().has_value() ? e.device_pixel_ratio().value() : 1;

    auto actual_x = pixel_scale * e.local_x().value();
    auto actual_y = pixel_scale * e.local_y().value();
    auto actual_component_name = e.component_name().value();

    EXPECT_NEAR(expected_x, actual_x, kEpsilon);
    EXPECT_NEAR(expected_y, actual_y, kEpsilon);
    EXPECT_EQ(actual_component_name, component_name);
  }

  void InjectInput(TapLocation tap_location) {
    // The /config/data/display_rotation (90) specifies how many degrees to rotate the
    // presentation child view, counter-clockwise, in a right-handed coordinate system. Thus,
    // the user observes the child view to rotate *clockwise* by that amount (90).
    //
    // Hence, a tap in the center of the display's top-right quadrant is observed by the child
    // view as a tap in the center of its top-left quadrant.
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

  // Routes needed to setup Chromium client.
  static std::vector<Route> GetWebEngineRoutes(ChildRef target) {
    return {
        {.capabilities =
             {Protocol{fidl::DiscoverableProtocolName<
                  fuchsia_accessibility_semantics::SemanticsManager>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_element::GraphicalPresenter>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Flatland>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Allocator>}},
         .source = kTestUIStackRef,
         .targets = {target}},
        {
            .capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>},
                             Dictionary{"diagnostics"}},
            .source = ParentRef(),
            .targets =
                {
                    target,
                    ChildRef{kFontsProvider},
                    ChildRef{kMemoryPressureSignaler},
                    ChildRef{kBuildInfoProvider},
                    ChildRef{kWebContextProvider},
                    ChildRef{kIntl},
                    ChildRef{kFakeCobalt},
                    // Not including kNetstack here, since it emits spurious
                    // FATAL errors.
                },
        },
        {.capabilities =
             {
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_kernel::VmexResource>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_process::Launcher>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_vulkan_loader::Loader>},
             },
         .source = ParentRef(),
         .targets = {target}},
        {.capabilities =
             {
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TouchInputListener>},
                 Protocol{
                     fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>},
             },
         .source = ChildRef{kMockResponseListener},
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
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_input::ImeService>}},
         .source = ChildRef{kTextManager},
         .targets = {target}},
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
             fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>}},
         .source = ParentRef(),
         .targets = {ChildRef{kFontsProvider}}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_metrics::MetricEventLoggerFactory>}},
         .source = ChildRef{kFakeCobalt},
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem::Allocator>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem2::Allocator>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}, target}},
        {.capabilities =
             {Protocol{fidl::DiscoverableProtocolName<fuchsia_kernel::RootJobForInspect>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_kernel::Stats>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_scheduler::RoleManager>},
              Protocol{fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_posix_socket::Provider>}},
         .source = ChildRef{kNetstack},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_buildinfo::Provider>}},
         .source = ChildRef{kBuildInfoProvider},
         .targets = {ChildRef{kWebContextProvider}, target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_intl::PropertyProvider>}},
         .source = ChildRef{kIntl},
         .targets = {target}},
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

  uint32_t display_rotation() override { return 90; }

  // TODO: https://fxbug.dev/42082519 - Test for DPR=2.0, too.
  float device_pixel_ratio() override { return 1.f; }

  std::shared_ptr<ResponseState> response_state() const { return response_state_; }

  static constexpr auto kOneChromiumClient = "chromium_client";
  static constexpr auto kOneChromiumUrl = "#meta/web-touch-input-chromium.cm";

 private:
  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |ResponseListener| service in the constructed realm.
    realm_builder().AddLocalChild(
        kMockResponseListener,
        [d = dispatcher(), response_state = response_state_, ready_to_inject = ready_to_inject_]() {
          return std::make_unique<ResponseListenerServer>(d, response_state, ready_to_inject);
        });
  }

  std::shared_ptr<ResponseState> response_state_ = std::make_shared<ResponseState>();
  std::shared_ptr<bool> ready_to_inject_ = std::make_shared<bool>(false);

  static constexpr auto kFontsProvider = "fonts_provider";
  static constexpr auto kFontsProviderUrl = "#meta/font_provider_hermetic_for_test.cm";

  static constexpr auto kTextManager = "text_manager";
  static constexpr auto kTextManagerUrl = "#meta/text_manager.cm";

  static constexpr auto kIntl = "intl";
  static constexpr auto kIntlUrl = "#meta/intl_property_manager.cm";

  static constexpr auto kMemoryPressureSignaler = "memory_pressure_signaler";
  static constexpr auto kMemoryPressureSignalerUrl = "#meta/memory_pressure_signaler.cm";

  static constexpr auto kNetstack = "netstack";
  static constexpr auto kNetstackUrl = "#meta/netstack.cm";

  static constexpr auto kWebContextProvider = "web_context_provider";
  static constexpr auto kWebContextProviderUrl =
      "fuchsia-pkg://fuchsia.com/web_engine#meta/context_provider.cm";

  static constexpr auto kBuildInfoProvider = "build_info_provider";
  static constexpr auto kBuildInfoProviderUrl = "#meta/fake_build_info.cm";

  static constexpr auto kFakeCobalt = "cobalt";
  static constexpr auto kFakeCobaltUrl = "#meta/fake_cobalt.cm";
};

INSTANTIATE_TEST_SUITE_P(WebEngineTestParameterized, WebEngineTest,
                         testing::ValuesIn(AsTuples(ConfigsToTest())));

TEST_P(WebEngineTest, ChromiumTap) {
  InjectInput(TapLocation::kTopLeft);

  RunLoopUntil([&] { return response_state()->events_received().size() >= 1; });

  ASSERT_EQ(response_state()->events_received().size(), 1u);

  ExpectLocation(response_state()->events_received()[0],
                 /*expected_x=*/static_cast<float>(display_height()) / 4.f,
                 /*expected_y=*/static_cast<float>(display_width()) / 4.f,
                 /*component_name=*/"web-touch-input-chromium");
}

}  // namespace
