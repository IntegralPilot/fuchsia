// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.conformance/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>

#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <src/lib/fsl/handles/object_info.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>
#include <src/ui/tests/conformance_input_tests/conformance-test-base.h>
#include <zxtest/zxtest.h>

namespace ui_conformance_testing {

const std::string PUPPET_UNDER_TEST_FACTORY_SERVICE = "/svc/puppet-under-test-factory-service";
const std::string AUXILIARY_PUPPET_FACTORY_SERVICE = "/svc/auxiliary-puppet-factory-service";

namespace futi = fuchsia_ui_test_input;
namespace futc = fuchsia_ui_test_conformance;
namespace fui = fuchsia_ui_input3;
namespace fuv = fuchsia_ui_views;
namespace fuf = fuchsia_ui_focus;

class FocusListener : public fidl::Server<fuf::FocusChainListener> {
 public:
  // |fuchsia_ui_focus::FocusChainListener|
  void OnFocusChange(OnFocusChangeRequest& request,
                     OnFocusChangeCompleter::Sync& completer) override {
    focus_chain_updates_.push_back(std::move(request.focus_chain()));
    completer.Reply();
  }

  fidl::ClientEnd<fuf::FocusChainListener> ServeAndGetClientEnd(async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<fuf::FocusChainListener>::Create();
    binding_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  bool IsViewFocused(const fuv::ViewRef& view_ref) const {
    if (focus_chain_updates_.empty()) {
      return false;
    }

    const auto& last_focus_chain = focus_chain_updates_.back();

    if (!last_focus_chain.focus_chain().has_value()) {
      return false;
    }

    if (last_focus_chain.focus_chain()->empty()) {
      return false;
    }

    // the new focus view store at the last slot.
    return fsl::GetKoid(last_focus_chain.focus_chain()->back().reference().get()) ==
           fsl::GetKoid(view_ref.reference().get());
  }

  const std::vector<fuf::FocusChain>& focus_chain_updates() const { return focus_chain_updates_; }

 private:
  fidl::ServerBindingGroup<fuf::FocusChainListener> binding_;
  std::vector<fuf::FocusChain> focus_chain_updates_;
};

class KeyListener : public fidl::Server<futi::KeyboardInputListener> {
 public:
  // |fuchsia_ui_test_input::KeyboardInputListener|
  void ReportTextInput(ReportTextInputRequest& request,
                       ReportTextInputCompleter::Sync& completer) override {
    events_received_.push_back(std::move(request));
  }

  // |fuchsia_ui_test_input::KeyboardInputListener|
  void ReportReady(ReportReadyCompleter::Sync& completer) override {
    // Puppet factory create view already wait for keyboard ready.
  }

  fidl::ClientEnd<futi::KeyboardInputListener> ServeAndGetClientEnd(
      async_dispatcher_t* dispatcher) {
    auto [client_end, server_end] = fidl::Endpoints<futi::KeyboardInputListener>::Create();
    binding_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
    return std::move(client_end);
  }

  const std::vector<futi::KeyboardInputListenerReportTextInputRequest>& events_received() {
    return events_received_;
  }

  void clear_events() { events_received_.clear(); }

 private:
  fidl::ServerBindingGroup<futi::KeyboardInputListener> binding_;
  std::vector<futi::KeyboardInputListenerReportTextInputRequest> events_received_;
};

// Holds resources associated with a single puppet instance.
struct KeyPuppet {
  fidl::SyncClient<futc::Puppet> client;
  KeyListener key_listener;
};

class KeyConformanceTest : public ui_conformance_test_base::ConformanceTest {
 public:
  ~KeyConformanceTest() override = default;

  void SetUp() override {
    ui_conformance_test_base::ConformanceTest::SetUp();

    // Register fake keyboard.
    {
      FX_LOGS(INFO) << "Connecting to input registry";
      auto input_registry = ConnectSyncIntoRealm<futi::Registry>();

      FX_LOGS(INFO) << "Registering fake keyboard";
      auto [client_end, server_end] = fidl::Endpoints<futi::Keyboard>::Create();

      futi::RegistryRegisterKeyboardAndGetDeviceInfoRequest request;
      request.device(std::move(server_end));
      auto res = input_registry->RegisterKeyboardAndGetDeviceInfo(std::move(request));
      ZX_ASSERT_OK(res);
      fake_keyboard_device_id_ = res->device_id().value();

      fake_keyboard_ = fidl::SyncClient(std::move(client_end));
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

    // Get root view token.
    {
      FX_LOGS(INFO) << "Creating root view token";

      auto controller = ConnectSyncIntoRealm<fuchsia_ui_test_scene::Controller>();

      fuchsia_ui_test_scene::ControllerPresentClientViewRequest req;
      auto [view_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
      req.viewport_creation_token(std::move(viewport_token));
      ZX_ASSERT_OK(controller->PresentClientView(std::move(req)));
      root_view_token_ = std::move(view_token);
    }

    // Setup focus listener.
    {
      auto focus_registry = ConnectSyncIntoRealm<fuf::FocusChainListenerRegistry>();
      auto focus_listener_client_end = focus_listener_.ServeAndGetClientEnd(dispatcher());

      ZX_ASSERT_OK(focus_registry->Register({std::move(focus_listener_client_end)}));
    }
  }

  void SimulateKeyEvent(std::string str) {
    FX_LOGS(INFO) << "Requesting key event";
    futi::KeyboardSimulateUsAsciiTextEntryRequest request;
    request.text(std::move(str));

    ZX_ASSERT_OK(fake_keyboard_->SimulateUsAsciiTextEntry(request));
    FX_LOGS(INFO) << "Key event injected";
  }

  void ExpectEvent(const futi::KeyboardInputListenerReportTextInputRequest& event,
                   std::optional<std::string> text,
                   std::optional<fui::NonPrintableKey> non_printable) const {
    ASSERT_EQ(text, event.text());
    ASSERT_EQ(non_printable, event.non_printable());
    ASSERT_EQ(fake_keyboard_device_id_, event.device_id());
  }

 protected:
  int32_t display_width_as_int() const { return static_cast<int32_t>(display_width_); }
  int32_t display_height_as_int() const { return static_cast<int32_t>(display_height_); }

  fidl::SyncClient<futi::Keyboard> fake_keyboard_;
  fuv::ViewCreationToken root_view_token_;
  FocusListener focus_listener_;
  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
  uint32_t fake_keyboard_device_id_ = 0;
};

class SingleViewKeyConformanceTest : public KeyConformanceTest {
 public:
  ~SingleViewKeyConformanceTest() override = default;

  void SetUp() override {
    KeyConformanceTest::SetUp();

    {
      FX_LOGS(INFO) << "Create puppet under test";
      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(PUPPET_UNDER_TEST_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fui::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      auto key_listener_client_end = puppet_.key_listener.ServeAndGetClientEnd(dispatcher());
      puppet_.client = fidl::SyncClient(std::move(puppet_client_end));

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token_));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.keyboard_listener(std::move(key_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);

      auto puppet_view_ref = std::move(res.value().view_ref().value());

      FX_LOGS(INFO) << "wait for focus";
      RunLoopUntil([&]() { return focus_listener_.IsViewFocused(puppet_view_ref); });
    }
  }

 protected:
  KeyPuppet puppet_;

 private:
  fidl::SyncClient<futc::PuppetFactory> puppet_factory_;
};

TEST_F(SingleViewKeyConformanceTest, SimpleKeyPress) {
  SimulateKeyEvent("Hello\nWorld!");
  FX_LOGS(INFO) << "Wait for puppet to report key events";
  RunLoopUntil([&]() { return puppet_.key_listener.events_received().size() >= 15; });

  ASSERT_EQ(puppet_.key_listener.events_received().size(), 15u);

  const auto& events = puppet_.key_listener.events_received();
  ExpectEvent(events[0], std::nullopt, fui::NonPrintableKey::kShift);
  // view reports value from key meaning so it is Shifted h (H).
  ExpectEvent(events[1], "H", std::nullopt);
  ExpectEvent(events[2], "e", std::nullopt);
  ExpectEvent(events[3], "l", std::nullopt);
  ExpectEvent(events[4], "l", std::nullopt);
  ExpectEvent(events[5], "o", std::nullopt);
  ExpectEvent(events[6], std::nullopt, fui::NonPrintableKey::kEnter);
  ExpectEvent(events[7], std::nullopt, fui::NonPrintableKey::kShift);
  ExpectEvent(events[8], "W", std::nullopt);
  ExpectEvent(events[9], "o", std::nullopt);
  ExpectEvent(events[10], "r", std::nullopt);
  ExpectEvent(events[11], "l", std::nullopt);
  ExpectEvent(events[12], "d", std::nullopt);
  ExpectEvent(events[13], std::nullopt, fui::NonPrintableKey::kShift);
  ExpectEvent(events[14], "!", std::nullopt);
}

class EmbeddedViewKeyConformanceTest : public KeyConformanceTest {
 public:
  ~EmbeddedViewKeyConformanceTest() override = default;

  void SetUp() override {
    KeyConformanceTest::SetUp();

    {
      FX_LOGS(INFO) << "Create parent puppet";

      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(AUXILIARY_PUPPET_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      parent_puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fui::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      auto key_listener_client_end = parent_puppet_.key_listener.ServeAndGetClientEnd(dispatcher());
      auto [focuser_client_end, focuser_server_end] = fidl::Endpoints<fuv::Focuser>::Create();
      parent_focuser_ = fidl::SyncClient(std::move(focuser_client_end));
      parent_puppet_.client = fidl::SyncClient(std::move(puppet_client_end));

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token_));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.keyboard_listener(std::move(key_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());
      creation_args.focuser(std::move(focuser_server_end));

      auto res = parent_puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);

      parent_view_ref_ = std::move(res.value().view_ref().value());
    }

    // Create child viewport.
    fuv::ViewCreationToken view_creation_token;
    {
      FX_LOGS(INFO) << "Creating child viewport";
      const uint64_t kChildViewportId = 1u;

      futc::ContentBounds bounds;
      bounds.size() = {display_width_ / 2, display_height_ / 2};
      bounds.origin() = {display_width_as_int() / 2, display_height_as_int() / 2};
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
          component::Connect<futc::PuppetFactory>(PUPPET_UNDER_TEST_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      child_puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fui::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();
      auto key_listener_client_end = child_puppet_.key_listener.ServeAndGetClientEnd(dispatcher());
      child_puppet_.client = fidl::SyncClient(std::move(puppet_client_end));

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(view_creation_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.keyboard_listener(std::move(key_listener_client_end));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = child_puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);

      child_view_ref_ = std::move(res.value().view_ref().value());
    }
  }

 protected:
  KeyPuppet parent_puppet_;
  fidl::SyncClient<fuv::Focuser> parent_focuser_;
  fuv::ViewRef parent_view_ref_;

  KeyPuppet child_puppet_;
  fuv::ViewRef child_view_ref_;

 private:
  fidl::SyncClient<futc::PuppetFactory> parent_puppet_factory_;
  fidl::SyncClient<futc::PuppetFactory> child_puppet_factory_;
};

TEST_F(EmbeddedViewKeyConformanceTest, KeyToFocusedView) {
  // Default is focus on parent view.
  {
    FX_LOGS(INFO) << "wait for focus to parent";
    RunLoopUntil([&]() { return focus_listener_.IsViewFocused(parent_view_ref_); });

    // Inject key events, and expect parent view receives events.
    SimulateKeyEvent("a");
    FX_LOGS(INFO) << "Wait for parent puppet to report key events";
    RunLoopUntil([&]() { return parent_puppet_.key_listener.events_received().size() >= 1; });

    ASSERT_EQ(parent_puppet_.key_listener.events_received().size(), 1u);

    const auto& events = parent_puppet_.key_listener.events_received();
    ExpectEvent(events[0], "a", std::nullopt);
    EXPECT_TRUE(child_puppet_.key_listener.events_received().empty());
    parent_puppet_.key_listener.clear_events();
  }

  // Focus on child view.
  {
    auto child_view_ref = scenic::cpp::CloneViewRef(child_view_ref_);

    ZX_ASSERT_OK(parent_focuser_->RequestFocus({std::move(child_view_ref)}));

    FX_LOGS(INFO) << "wait for focus to child";
    RunLoopUntil([&]() { return focus_listener_.IsViewFocused(child_view_ref_); });

    // Inject key events, and expect child view receives events.
    SimulateKeyEvent("b");

    FX_LOGS(INFO) << "Wait for child puppet to report key events";
    RunLoopUntil([&]() { return child_puppet_.key_listener.events_received().size() >= 1; });

    ASSERT_EQ(child_puppet_.key_listener.events_received().size(), 1u);

    const auto& events = child_puppet_.key_listener.events_received();
    ExpectEvent(events[0], "b", std::nullopt);
    EXPECT_TRUE(parent_puppet_.key_listener.events_received().empty());
    child_puppet_.key_listener.clear_events();
  }

  // Focus back to parent view.
  {
    auto parent_view_ref = scenic::cpp::CloneViewRef(parent_view_ref_);

    ZX_ASSERT_OK(parent_focuser_->RequestFocus({std::move(parent_view_ref)}));

    FX_LOGS(INFO) << "wait for focus to parent";
    RunLoopUntil([&]() { return focus_listener_.IsViewFocused(parent_view_ref_); });

    // Inject key events, and expect parent view receives events.
    SimulateKeyEvent("c");
    FX_LOGS(INFO) << "Wait for parent puppet to report key events";
    RunLoopUntil([&]() { return parent_puppet_.key_listener.events_received().size() >= 1; });

    ASSERT_EQ(parent_puppet_.key_listener.events_received().size(), 1u);

    const auto& events = parent_puppet_.key_listener.events_received();
    ExpectEvent(events[0], "c", std::nullopt);
    EXPECT_TRUE(child_puppet_.key_listener.events_received().empty());
    parent_puppet_.key_listener.clear_events();
  }
}

}  //  namespace ui_conformance_testing
