// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <fidl/fuchsia.cobalt/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.fonts/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fidl/fuchsia.metrics/cpp/fidl.h>
#include <fidl/fuchsia.net.interfaces/cpp/fidl.h>
#include <fidl/fuchsia.posix.socket/cpp/fidl.h>
#include <fidl/fuchsia.process/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.session.scene/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <fidl/fuchsia.web/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/stdcompat/string_view.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <zircon/status.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <cstddef>
#include <iostream>
#include <memory>
#include <optional>
#include <utility>
#include <vector>

#include <gtest/gtest.h>
#include <src/lib/fsl/handles/object_info.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/portable_ui_test.h>

namespace {

// Types imported for the realm_builder library.
using component_testing::ChildRef;
using component_testing::Directory;
using component_testing::LocalComponentImpl;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::Route;

using ChildName = std::string;

// Max timeout in failure cases.
// Set this as low as you can that still works across all test platforms.
constexpr zx::duration kTimeout = zx::min(5);

// Externalized test specific keyboard input state.
//
// A shsared instance of this object is kept in the test fixture, and also
// injected into KeyboardInputListener below.
class KeyboardInputState {
 public:
  // If true, the web app is ready to handling input events.
  bool IsReadyForInput() const { return ready_for_input_; }

  // If true, the web app is ready to handling key events.
  bool IsReadyForKey() const { return ready_for_key_; }

  // Returns true if the last response received matches `expected`.  If a match is found,
  // the match is consumed, so a next call to HasResponse starts from scratch.
  bool HasResponse(const std::string& expected) {
    bool match = response_.has_value() && response_.value() == expected;
    if (match) {
      response_ = std::nullopt;
    }
    return match;
  }

  // Same as above, except we are looking for a substring.
  bool ResponseContains(const std::string& substring) {
    bool match = response_.has_value() && cpp23::contains(response_.value(), substring);
    if (match) {
      response_ = std::nullopt;
    }
    return match;
  }

 private:
  friend class KeyboardInputListenerServer;

  std::optional<std::string> response_ = std::nullopt;
  bool ready_for_input_ = false;
  bool ready_for_key_ = false;
};

// `KeyboardInputListener` is a test protocol that our test app uses to let us know
// what text is being entered into its only text field.
//
// The text field contents are reported on almost every change, so if you are entering a long
// text, you will see calls corresponding to successive additions of characters, not just the
// end result.
class KeyboardInputListenerServer
    : public fidl::Server<fuchsia_ui_test_input::KeyboardInputListener>,
      public fidl::Server<fuchsia_ui_test_input::TestAppStatusListener>,
      public LocalComponentImpl {
 public:
  KeyboardInputListenerServer(async_dispatcher_t* dispatcher,
                              std::weak_ptr<KeyboardInputState> state)
      : dispatcher_(dispatcher), state_(std::move(state)) {}

  KeyboardInputListenerServer(const KeyboardInputListenerServer&) = delete;
  KeyboardInputListenerServer& operator=(const KeyboardInputListenerServer&) = delete;

  // |fuchsia_ui_test_input::KeyboardInputListener|
  void ReportTextInput(ReportTextInputRequest& request,
                       ReportTextInputCompleter::Sync& completer) override {
    FX_LOGS(INFO) << "App sent: '" << request.text().value() << "'";
    if (auto s = state_.lock()) {
      s->response_ = request.text();
    }
  }

  // |fuchsia_ui_test_input::KeyboardInputListener|
  void ReportReady(ReportReadyCompleter::Sync& completer) override {
    // Deprecated
  }

  void ReportStatus(ReportStatusRequest& req, ReportStatusCompleter::Sync& completer) override {
    if (req.status() == fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered) {
      if (auto state = state_.lock()) {
        state->ready_for_input_ = true;
      }
    } else if (req.status() == fuchsia_ui_test_input::TestAppStatus::kElementFocused) {
      if (auto state = state_.lock()) {
        state->ready_for_key_ = true;
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

  // Starts this server.
  void OnStart() override {
    outgoing()->AddProtocol<fuchsia_ui_test_input::KeyboardInputListener>(
        keyboard_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    outgoing()->AddProtocol<fuchsia_ui_test_input::TestAppStatusListener>(
        app_status_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  }

 private:
  // Not owned.
  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::KeyboardInputListener> keyboard_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TestAppStatusListener> app_status_bindings_;
  std::weak_ptr<KeyboardInputState> state_;
};

constexpr auto kResponseListener = "test_text_response_listener";

// See README.md for instructions on how to run this test with chrome remote
// devtools, which is super-useful for debugging.
class ChromiumInputBase : public ui_testing::PortableUITest {
 protected:
  ChromiumInputBase() : keyboard_input_state_(std::make_shared<KeyboardInputState>()) {}

  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  void SetUp() override {
    ui_testing::PortableUITest::SetUp();

    // Post a "just in case" quit task, if the test hangs.
    async::PostDelayedTask(
        dispatcher(),
        [] { FX_LOGS(FATAL) << "\n\n>> Test did not complete in time, terminating.  <<\n\n"; },
        kTimeout);

    RegisterTouchScreen();
    RegisterKeyboard();
  }

  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |ResponseListener| service in the constructed realm.
    realm_builder().AddLocalChild(kResponseListener,
                                  [d = dispatcher(), s = keyboard_input_state_]() {
                                    return std::make_unique<KeyboardInputListenerServer>(d, s);
                                  });
  }

  // Needs to outlive realm_ below.
  std::shared_ptr<KeyboardInputState> keyboard_input_state_;

  std::optional<async::Task> inject_retry_task_;
};

class ChromiumInputTest : public ChromiumInputBase {
 protected:
  static constexpr auto kTextInputChromium = "text-input-chromium";
  static constexpr auto kTextInputChromiumUrl = "#meta/text-input-chromium.cm";

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

  static constexpr auto kFontsProvider = "fonts_provider";
  static constexpr auto kFontsProviderUrl = "#meta/font_provider_hermetic_for_test.cm";

  static constexpr auto kIntl = "intl";
  static constexpr auto kIntlUrl = "#meta/intl_property_manager.cm";

  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {
        std::make_pair(kTextInputChromium, kTextInputChromiumUrl),
    };
  }

  std::vector<std::pair<ChildName, std::string>> GetTestComponents() override {
    return {
        std::make_pair(kBuildInfoProvider, kBuildInfoProviderUrl),
        std::make_pair(kMemoryPressureSignaler, kMemoryPressureSignalerUrl),
        std::make_pair(kNetstack, kNetstackUrl),
        std::make_pair(kFakeCobalt, kFakeCobaltUrl),
        std::make_pair(kFontsProvider, kFontsProviderUrl),
        std::make_pair(kIntl, kIntlUrl),
        std::make_pair(kWebContextProvider, kWebContextProviderUrl),
    };
  }

  std::vector<Route> GetTestRoutes() override {
    return GetChromiumRoutes(ChildRef{kTextInputChromium});
  }

  // Routes needed to setup Chromium client.
  static std::vector<Route> GetChromiumRoutes(ChildRef target) {
    auto r_star_dir = fuchsia_io::kRStarDir;
    return {
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
                    ChildRef{kIntl},
                    ChildRef{kFakeCobalt},
                    // Not including kNetstack here, since it emits spurious
                    // FATAL errors.
                },
        },
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
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_fonts::Provider>}},
         .source = ChildRef{kFontsProvider},
         .targets = {target}},
        {.capabilities = {Protocol{
                              fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>},
                          Directory{.name = "config-data",
                                    .rights = fidl::NaturalToHLCPP(r_star_dir),
                                    .path = "/config/data"}},
         .source = ParentRef(),
         .targets = {ChildRef{kFontsProvider}, ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_intl::PropertyProvider>}},
         .source = ChildRef{kIntl},
         .targets = {target}},
        {.capabilities =
             {Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_ui_test_input::KeyboardInputListener>},
              Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>}},
         .source = ChildRef{kResponseListener},
         .targets = {target}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_memorypressure::Provider>}},
         .source = ChildRef{kMemoryPressureSignaler},
         .targets = {target}},

        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_posix_socket::Provider>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_net_interfaces::State>}},
         .source = ChildRef{kNetstack},
         // Use the .source below instead of above,
         // if you want to use Chrome remote debugging. See README.md for
         // instructions.
         //.source = ParentRef(),
         .targets = {target}},

        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_web::ContextProvider>}},
         .source = ChildRef{kWebContextProvider},
         .targets = {target}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_metrics::MetricEventLoggerFactory>}},
         .source = ChildRef{kFakeCobalt},
         .targets = {ChildRef{kMemoryPressureSignaler}}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_input3::Keyboard>}},
         .source = kTestUIStackRef,
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem::Allocator>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_sysmem2::Allocator>}},
         .source = ParentRef(),
         .targets = {target, ChildRef{kMemoryPressureSignaler}}},
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
         .targets = {target, ChildRef{kMemoryPressureSignaler}}},
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

  void LaunchWebEngineClient() {
    WaitForViewPresentation();

    FX_LOGS(INFO) << "Waiting on app ready to handle input events";
    RunLoopUntil([this]() { return keyboard_input_state_->IsReadyForInput(); });

    FX_LOGS(INFO) << "Waiting on app focused on textarea: ready for key events";
    const int kMaxRetries = 10;
    for (int i = 0; i < kMaxRetries; i++) {
      FX_LOGS(INFO) << "Tapping on textarea: " << i + 1 << "/" << kMaxRetries;
      // TODO(b/477639582): Repeating the tap may not be necessary, but adding
      // it for now to stable the test after chromium 145.0.7622.0 roll.
      // Not quite exactly the location of the text area under test, but since the
      // text area occupies all the screen, it's very likely within the text area.
      InjectTap(display_width() / 2, display_height() / 2);

      RunLoopWithTimeoutOrUntil([this]() { return keyboard_input_state_->IsReadyForKey(); },
                                zx::sec(1));
      if (keyboard_input_state_->IsReadyForKey()) {
        FX_LOGS(INFO) << "App is ready for key events: tapped " << i + 1 << " times";
        break;
      }
    }
    // After 10 taps, the app should be ready for key events.
    ASSERT_TRUE(keyboard_input_state_->IsReadyForKey());
  }
};

// Launches a web engine to opens a page with a full-screen text input window.
// Then taps the screen to move focus to that page, and types text on the
// fake injected keyboard.  Loops around until the text appears in the
// text area.
TEST_F(ChromiumInputTest, BasicInputTest) {
  LaunchWebEngineClient();

  SimulateUsAsciiTextEntry("Hello\nworld!");

  RunLoopUntil([&] { return keyboard_input_state_->ResponseContains("Hello\\nworld!"); });

  FX_LOGS(INFO) << "Done";
}

}  // namespace
