// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.input3/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.conformance/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.scene/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>

#include <string>
#include <utility>
#include <vector>

#include <src/lib/fsl/handles/object_info.h>
#include <src/ui/tests/conformance_input_tests/conformance-test-base.h>
#include <zxtest/zxtest.h>

namespace ui_conformance_testing {

const std::string PUPPET_UNDER_TEST_FACTORY_SERVICE = "/svc/puppet-under-test-factory-service";

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

class FocusConformanceTest : public ui_conformance_test_base::ConformanceTest {
 public:
  void PresentAndAddPuppetView() {
    // Present view.
    fuv::ViewCreationToken root_view_token;
    {
      FX_LOGS(INFO) << "Creating root view token";

      auto controller = ConnectSyncIntoRealm<fuchsia_ui_test_scene::Controller>();

      fuchsia_ui_test_scene::ControllerPresentClientViewRequest req;
      auto [view_token, viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
      req.viewport_creation_token(std::move(viewport_token));
      ZX_ASSERT_OK(controller->PresentClientView(std::move(req)));
      root_view_token = std::move(view_token);
    }

    // Add puppet view.
    {
      FX_LOGS(INFO) << "Create puppet under test";

      auto puppet_factory_connect =
          component::Connect<futc::PuppetFactory>(PUPPET_UNDER_TEST_FACTORY_SERVICE);
      ZX_ASSERT_OK(puppet_factory_connect);

      puppet_factory_ = fidl::SyncClient(std::move(puppet_factory_connect.value()));

      auto flatland = ConnectIntoRealm<fuchsia_ui_composition::Flatland>();
      auto keyboard = ConnectIntoRealm<fui::Keyboard>();
      auto [puppet_client_end, puppet_server_end] = fidl::Endpoints<futc::Puppet>::Create();

      futc::PuppetCreationArgs creation_args;
      creation_args.server_end(std::move(puppet_server_end));
      creation_args.view_token(std::move(root_view_token));
      creation_args.flatland_client(std::move(flatland));
      creation_args.keyboard_client(std::move(keyboard));
      creation_args.device_pixel_ratio(DevicePixelRatio());

      auto res = puppet_factory_->Create(std::move(creation_args));
      ZX_ASSERT_OK(res);
      ASSERT_EQ(res.value().result(), futc::Result::kSuccess);

      puppet_view_ref_ = std::move(res.value().view_ref().value());
    }
  }

  fuv::ViewRef puppet_view_ref_;

 private:
  fidl::SyncClient<futc::PuppetFactory> puppet_factory_;
};

// This test exercises the focus contract with the scene owner: the view offered to the
// scene owner will have focus transferred to it.
TEST_F(FocusConformanceTest, ReceivesFocusTransfer) {
  // Setup focus listener.
  FocusListener focus_listener;
  {
    auto focus_registry = ConnectSyncIntoRealm<fuf::FocusChainListenerRegistry>();
    auto focus_listener_client_end = focus_listener.ServeAndGetClientEnd(dispatcher());

    ZX_ASSERT_OK(focus_registry->Register({std::move(focus_listener_client_end)}));
  }

  RunLoopUntil([&focus_listener]() { return focus_listener.focus_chain_updates().size() > 0; });

  // No focus chain when no scene.
  EXPECT_FALSE(focus_listener.focus_chain_updates().back().focus_chain().has_value());

  PresentAndAddPuppetView();

  FX_LOGS(INFO) << "wait for focus";
  RunLoopUntil([&]() { return focus_listener.IsViewFocused(puppet_view_ref_); });
}

// This test ensures that multiple clients can connect to the FocusChainListenerRegistry.
TEST_F(FocusConformanceTest, MultiListener) {
  // Setup focus listener a.
  FocusListener focus_listener_a;
  {
    auto focus_registry = ConnectSyncIntoRealm<fuf::FocusChainListenerRegistry>();
    auto focus_listener_client_end = focus_listener_a.ServeAndGetClientEnd(dispatcher());

    ZX_ASSERT_OK(focus_registry->Register({std::move(focus_listener_client_end)}));
  }

  RunLoopUntil([&focus_listener_a]() { return focus_listener_a.focus_chain_updates().size() > 0; });

  // No focus chain when no scene.
  EXPECT_FALSE(focus_listener_a.focus_chain_updates().back().focus_chain().has_value());

  // Setup focus listener b.
  FocusListener focus_listener_b;
  {
    auto focus_registry = ConnectSyncIntoRealm<fuf::FocusChainListenerRegistry>();
    auto focus_listener_client_end = focus_listener_b.ServeAndGetClientEnd(dispatcher());

    ZX_ASSERT_OK(focus_registry->Register({std::move(focus_listener_client_end)}));
  }

  PresentAndAddPuppetView();

  FX_LOGS(INFO) << "focus listener a receives focus transfer";
  RunLoopUntil([&]() { return focus_listener_a.IsViewFocused(puppet_view_ref_); });

  FX_LOGS(INFO) << "focus listener b receives focus transfer";
  RunLoopUntil([&]() { return focus_listener_b.IsViewFocused(puppet_view_ref_); });
}

}  //  namespace ui_conformance_testing
