// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.accessibility.semantics/cpp/fidl.h>
#include <fidl/fuchsia.buildinfo/cpp/fidl.h>
#include <fidl/fuchsia.component.decl/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.fonts/cpp/fidl.h>
#include <fidl/fuchsia.input.injection/cpp/fidl.h>
#include <fidl/fuchsia.input.virtualkeyboard/cpp/fidl.h>
#include <fidl/fuchsia.intl/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
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
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.vulkan.loader/cpp/fidl.h>
#include <fidl/fuchsia.web/cpp/fidl.h>
#include <fidl/test.virtualkeyboard/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/types.h>
#include <zircon/utc.h>

#include <iostream>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>
#include <src/lib/timekeeper/clock.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/testing/util/portable_ui_test.h>

// This test exercises the virtual keyboard visibility interactions between
// Chromium and Virtual Keyboard Manager. It is a multi-component test, and
// carefully avoids sleeping or polling for component coordination.
// - It runs real Virtual Keyboard Manager, Scene Manager and Scenic components.
// - It uses a fake display controller; the physical device is unused.
//
// Components involved
// - This test program
// - Virtual Keyboard Manager
// - Scene Manager (serves root view and touch input)
// - Scenic
// - WebEngine (built from Chromium)
//
// Setup sequence
// - The test sets up a view hierarchy with two views:
//   - Top level scene, owned by Scene Manager.
//   - Bottom view, owned by Chromium.

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

constexpr auto kResponseListener = "response_listener";

using time_utc = timekeeper::time_utc;

class InputPositionState {
 public:
  const std::optional<test_virtualkeyboard::BoundingBox>& input_position() const {
    return input_position_;
  }

  // If true, the web app is ready to handling input events.
  bool IsReadyForInput() const { return ready_for_input_; }

 private:
  friend class InputPositionListenerServer;
  std::optional<test_virtualkeyboard::BoundingBox> input_position_;
  bool ready_for_input_ = false;
};

// This component implements the interface for a RealmBuilder
// LocalComponent and the test.virtualkeyboard.InputPositionListener
// protocol.
class InputPositionListenerServer
    : public fidl::Server<test_virtualkeyboard::InputPositionListener>,
      public fidl::Server<fuchsia_ui_test_input::TestAppStatusListener>,
      public LocalComponentImpl {
 public:
  explicit InputPositionListenerServer(async_dispatcher_t* dispatcher,
                                       std::weak_ptr<InputPositionState> state)
      : dispatcher_(dispatcher), state_(std::move(state)) {}

  // |test_virtualkeyboard::InputPositionListener|
  void Notify(NotifyRequest& req, NotifyCompleter::Sync& completer) override {
    if (auto state = state_.lock()) {
      state->input_position_ = req.bounding_box();
    }
  }

  void ReportStatus(ReportStatusRequest& req, ReportStatusCompleter::Sync& completer) override {
    if (req.status() == fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered) {
      if (auto state = state_.lock()) {
        state->ready_for_input_ = true;
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

  // |LocalComponentImpl::OnStart|
  void OnStart() override {
    outgoing()->AddProtocol<test_virtualkeyboard::InputPositionListener>(
        position_listener_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
    outgoing()->AddProtocol<fuchsia_ui_test_input::TestAppStatusListener>(
        app_status_bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  }

 private:
  async_dispatcher_t* dispatcher_ = nullptr;
  fidl::ServerBindingGroup<test_virtualkeyboard::InputPositionListener> position_listener_bindings_;
  fidl::ServerBindingGroup<fuchsia_ui_test_input::TestAppStatusListener> app_status_bindings_;
  std::weak_ptr<InputPositionState> state_;
};

class VirtualKeyboardTest : public ui_testing::PortableUITest {
 protected:
  void SetUp() override {
    ui_testing::PortableUITest::SetUp();
    RegisterTouchScreen();
  }

  void LaunchWebEngineClient() {
    WaitForViewPresentation();

    FX_LOGS(INFO) << "Waiting on app ready to handle input events";
    RunLoopUntil([this]() { return input_position_state_->IsReadyForInput(); });
  }

  std::string GetTestUIStackUrl() override { return "#meta/test-ui-stack.cm"; }

  void ExtendRealm() override {
    // Key part of service setup: have this test component vend the
    // |ResponseListener| service in the constructed realm.
    realm_builder().AddLocalChild(kResponseListener,
                                  [d = dispatcher(), s = input_position_state_]() {
                                    return std::make_unique<InputPositionListenerServer>(d, s);
                                  });
  }

  std::vector<std::pair<ChildName, std::string>> GetEagerTestComponents() override {
    return {
        std::make_pair(kWebVirtualKeyboardClient, kWebVirtualKeyboardUrl),
    };
  }

  std::vector<std::pair<ChildName, std::string>> GetTestComponents() override {
    return {
        std::make_pair(kBuildInfoProvider, kBuildInfoProviderUrl),
        std::make_pair(kFontsProvider, kFontsProviderUrl),
        std::make_pair(kIntl, kIntlUrl),
        std::make_pair(kMemoryPressureSignaler, kMemoryPressureSignalerUrl),
        std::make_pair(kFakeCobalt, kFakeCobaltUrl),
        std::make_pair(kNetstack, kNetstackUrl),
        std::make_pair(kWebContextProvider, kWebContextProviderUrl),
    };
  }

  std::vector<Route> GetTestRoutes() override {
    return GetChromiumRoutes(ChildRef{kWebVirtualKeyboardClient});
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
             {Protocol{fidl::DiscoverableProtocolName<test_virtualkeyboard::InputPositionListener>},
              Protocol{
                  fidl::DiscoverableProtocolName<fuchsia_ui_test_input::TestAppStatusListener>}},
         .source = ChildRef{kResponseListener},
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
         .targets = {ChildRef{kFontsProvider}}},
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
                 Protocol{fidl::DiscoverableProtocolName<
                     fuchsia_accessibility_semantics::SemanticsManager>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_element::GraphicalPresenter>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_input3::Keyboard>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Allocator>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_composition::Flatland>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_ui_input::ImeService>},
                 Protocol{fidl::DiscoverableProtocolName<fuchsia_input_virtualkeyboard::Manager>},
                 Protocol{fidl::DiscoverableProtocolName<
                     fuchsia_input_virtualkeyboard::ControllerCreator>},
             },
         .source = kTestUIStackRef,
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_intl::PropertyProvider>}},
         .source = ChildRef{kIntl},
         .targets = {target}},
        {.capabilities = {Protocol{
             fidl::DiscoverableProtocolName<fuchsia_memorypressure::Provider>}},
         .source = ChildRef{kMemoryPressureSignaler},
         .targets = {target}},
        {.capabilities = {Protocol{fidl::DiscoverableProtocolName<fuchsia_posix_socket::Provider>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_net_interfaces::State>}},
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
        {.capabilities = {Protocol{
                              fidl::DiscoverableProtocolName<fuchsia_tracing_provider::Registry>},
                          Protocol{fidl::DiscoverableProtocolName<fuchsia_scheduler::RoleManager>}},
         .source = ParentRef(),
         .targets = {ChildRef{kMemoryPressureSignaler}}},
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

  std::shared_ptr<InputPositionState> input_position_state() const { return input_position_state_; }

 private:
  std::shared_ptr<InputPositionState> input_position_state_ =
      std::make_shared<InputPositionState>();

  static constexpr auto kFakeCobalt = "cobalt";
  static constexpr auto kFakeCobaltUrl = "#meta/fake_cobalt.cm";

  static constexpr auto kWebVirtualKeyboardClient = "web_virtual_keyboard_client";
  static constexpr auto kWebVirtualKeyboardUrl = "#meta/web-virtual-keyboard-client.cm";

  static constexpr auto kFontsProvider = "fonts_provider";
  static constexpr auto kFontsProviderUrl = "#meta/font_provider_hermetic_for_test.cm";

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
};

TEST_F(VirtualKeyboardTest, ShowAndHideKeyboard) {
  LaunchWebEngineClient();

  std::optional<bool> is_keyboard_visible;
  auto virtualkeyboard_manager_connect =
      realm_root()->component().Connect<fuchsia_input_virtualkeyboard::Manager>();
  ZX_ASSERT_OK(virtualkeyboard_manager_connect);
  auto virtualkeyboard_manager =
      fidl::Client(std::move(virtualkeyboard_manager_connect.value()), dispatcher());

  virtualkeyboard_manager->WatchTypeAndVisibility().Then(
      [&is_keyboard_visible](auto& res) { is_keyboard_visible = res->is_visible(); });
  RunLoopUntil([&]() { return is_keyboard_visible.has_value(); });
  // virtual keyboard should not be visible before tap.
  ASSERT_FALSE(is_keyboard_visible.value());
  is_keyboard_visible.reset();

  FX_LOGS(INFO) << "Getting input box position";
  RunLoopUntil([this]() { return input_position_state()->input_position().has_value(); });

  FX_LOGS(INFO) << "Tapping _inside_ input box";
  auto input_pos = *input_position_state()->input_position();
  int32_t input_center_x_local = static_cast<int32_t>((input_pos.x0() + input_pos.x1()) / 2);
  int32_t input_center_y_local = static_cast<int32_t>((input_pos.y0() + input_pos.y1()) / 2);

  // TODO(b/477639582): Repeating the tap may not be necessary, but adding
  // it for now to stable the test after chromium 145.0.7622.0 roll.
  virtualkeyboard_manager->WatchTypeAndVisibility().Then(
      [&is_keyboard_visible](auto& res) { is_keyboard_visible = res->is_visible(); });

  const int kMaxRetries = 10;

  for (int i = 0; i < kMaxRetries; i++) {
    FX_LOGS(INFO) << "Tapping: " << i + 1 << "/" << kMaxRetries;
    InjectTap(input_center_x_local, input_center_y_local);
    FX_LOGS(INFO) << "Waiting for keyboard to be visible";
    RunLoopWithTimeoutOrUntil([&]() { return is_keyboard_visible.has_value(); }, zx::sec(1));
    if (is_keyboard_visible.has_value()) {
      FX_LOGS(INFO) << "Keyboard is visible: tapped " << i + 1 << " times";
      break;
    }
  }

  EXPECT_TRUE(is_keyboard_visible.value());
  is_keyboard_visible.reset();

  FX_LOGS(INFO) << "Tapping _outside_ input box";
  InjectTap(static_cast<int32_t>(input_pos.x1() + 1), static_cast<int32_t>(input_pos.y1() + 1));

  FX_LOGS(INFO) << "Waiting for keyboard to be hidden";
  virtualkeyboard_manager->WatchTypeAndVisibility().Then(
      [&is_keyboard_visible](auto& res) { is_keyboard_visible = res->is_visible(); });
  RunLoopUntil([&]() { return is_keyboard_visible.has_value(); });
  EXPECT_FALSE(is_keyboard_visible.value());
}

}  // namespace
