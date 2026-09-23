// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <zircon/status.h>

#include <memory>
#include <optional>
#include <utility>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"

// This test exercises the focus protocols implemented by Scenic (fuchsia.ui.focus.FocusChain,
// fuchsia.ui.views.Focuser, fuchsia.ui.views.ViewRefFocused) in the context of the Flatland
// compositor interface. The geometry is not important in this test, so we use the following
// two-node tree topology:
//    parent
//      |
//    child
namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuf = fuchsia_ui_focus;
namespace fuv = fuchsia_ui_views;

#define EXPECT_VIEW_REF_MATCH(view_ref1, view_ref2) \
  EXPECT_EQ(ExtractKoid(view_ref1), ExtractKoid(view_ref2))

using ChildViewWatcher = fuc::ChildViewWatcher;
using ContentId = fuc::ContentId;
using Flatland = fuc::Flatland;
using FlatlandDisplay = fuc::FlatlandDisplay;
using ParentViewportWatcher = fuc::ParentViewportWatcher;
using TransformId = fuc::TransformId;
using ViewBoundProtocols = fuc::ViewBoundProtocols;
using ViewportProperties = fuc::ViewportProperties;
using FocusChain = fuf::FocusChain;
using FocusChainListener = fuf::FocusChainListener;
using ViewCreationToken = fuv::ViewCreationToken;
using ViewportCreationToken = fuv::ViewportCreationToken;
using ViewRef = fuv::ViewRef;

namespace {

// "Long enough" time to wait before assuming updates won't arrive.
// Should not be used when actually expecting an update to occur.
const zx::duration kWaitTime = zx::msec(100);
const uint32_t kDefaultLogicalPixelSize = 1;
const fuc::TransformId kRootTransform(1);

}  // namespace

class FlatlandFocusIntegrationTest : public ScenicCtfTest,
                                     public fidl::Server<fuf::FocusChainListener> {
 protected:
  FlatlandFocusIntegrationTest() = default;

  void SetUp() override {
    ScenicCtfTest::SetUp();

    // Set up focus chain listener and wait for the initial null focus chain.
    auto [listener_client_end, listener_server_end] =
        fidl::CreateEndpoints<fuf::FocusChainListener>().value();
    focus_chain_listener_binding_.emplace(dispatcher(), std::move(listener_server_end), this,
                                          fidl::kIgnoreBindingClosure);
    fidl::SyncClient focus_chain_listener_registry =
        ConnectSyncIntoRealm<fuf::FocusChainListenerRegistry>();
    EXPECT_TRUE(
        focus_chain_listener_registry->Register({{.listener = std::move(listener_client_end)}})
            .is_ok());
    EXPECT_EQ(CountReceivedFocusChains(), 0u);
    RunLoopUntil([this] { return CountReceivedFocusChains() >= 1u; });
    observed_focus_chains_.clear();

    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));

    // Set up root view.
    root_session_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_session_->set_on_close(FailOnClose("Lost connection to Scenic"));

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    root_view_ref_ = scenic::cpp::CloneViewRef(identity.view_ref());

    auto [root_focuser_client_end, root_focuser_server_end] =
        fidl::CreateEndpoints<fuv::Focuser>().value();
    root_focuser_ = std::make_unique<SimpleWatcherClient<fuv::Focuser>>(
        std::move(root_focuser_client_end), dispatcher());

    auto [root_focused_client_end, root_focused_server_end] =
        fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
    root_focused_ = std::make_unique<SimpleWatcherClient<fuv::ViewRefFocused>>(
        std::move(root_focused_client_end), dispatcher());

    ViewBoundProtocols protocols;
    protocols.view_focuser(std::move(root_focuser_server_end));
    protocols.view_ref_focused(std::move(root_focused_server_end));

    EXPECT_TRUE((*root_session_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *root_session_);

    // Now that the scene exists, wait for a valid focus chain. It should only contain the root
    // view.
    RunLoopUntil([this] { return CountReceivedFocusChains() >= 1u; });
    EXPECT_TRUE(LastFocusChain()->focus_chain().has_value());
    ASSERT_EQ(LastFocusChain()->focus_chain()->size(), 1u);
    EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain()->front(), root_view_ref_);

    // And the root's ViewRefFocused Watch call should fire, since it is now focused.
    bool root_focused = false;
    (*root_focused_)
        ->Watch()
        .Then([&root_focused](fidl::Result<fuv::ViewRefFocused::Watch>& update) {
          ASSERT_TRUE(update.is_ok());
          ASSERT_TRUE(update->state().focused().has_value());
          root_focused = update->state().focused().value();
        });
    RunLoopUntil([&root_focused] { return root_focused; });

    observed_focus_chains_.clear();
  }

  bool RequestFocusChange(SimpleWatcherClient<fuv::Focuser>& view_focuser_ptr,
                          const ViewRef& target) {
    FX_CHECK(view_focuser_ptr.is_bound());
    bool request_processed = false;
    bool request_honored = false;
    view_focuser_ptr->RequestFocus({{.view_ref = scenic::cpp::CloneViewRef(target)}})
        .Then([&request_processed,
               &request_honored](fidl::Result<fuv::Focuser::RequestFocus>& result) {
          request_processed = true;
          if (result.is_ok()) {
            request_honored = true;
          }
        });
    RunLoopUntil([&request_processed] { return request_processed; });
    return request_honored;
  }

  void SetAutoFocus(SimpleWatcherClient<fuv::Focuser>& view_focuser_ptr, const ViewRef& target) {
    bool request_processed = false;
    view_focuser_ptr->SetAutoFocus({{.view_ref = scenic::cpp::CloneViewRef(target)}})
        .Then([&request_processed](fidl::Result<fuv::Focuser::SetAutoFocus>& result) {
          request_processed = true;
          if (result.is_error()) {
            FAIL();
          }
        });
    RunLoopUntil([&request_processed] { return request_processed; });
  }

  void AttachToRoot(ViewportCreationToken&& token) {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    fuc::ViewportProperties properties;
    properties.logical_size(
        fuchsia_math::SizeU(kDefaultLogicalPixelSize, kDefaultLogicalPixelSize));
    const ContentId kRootContent(1);
    EXPECT_TRUE((*root_session_)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE(
        (*root_session_)
            ->CreateViewport({{.viewport_id = kRootContent,
                               .token = std::move(token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE((*root_session_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*root_session_)
                    ->SetContent({{.transform_id = kRootTransform, .content_id = kRootContent}})
                    .is_ok());
    BlockingPresent(this, *root_session_);
  }

  // |fidl::Server<fuchsia_ui_focus::FocusChainListener>|
  void OnFocusChange(fuf::FocusChainListenerOnFocusChangeRequest& request,
                     OnFocusChangeCompleter::Sync& completer) override {
    observed_focus_chains_.push_back(std::move(request.focus_chain()));
    completer.Reply();
  }

  size_t CountReceivedFocusChains() const { return observed_focus_chains_.size(); }

  const FocusChain* LastFocusChain() const {
    if (observed_focus_chains_.empty()) {
      return nullptr;
    }
    return &observed_focus_chains_.back();
  }

  std::unique_ptr<FlatlandClientWithEventHandler> root_session_;
  fuv::ViewRef root_view_ref_;
  std::unique_ptr<SimpleWatcherClient<fuv::Focuser>> root_focuser_;
  std::unique_ptr<SimpleWatcherClient<fuv::ViewRefFocused>> root_focused_;

 private:
  std::vector<FocusChain> observed_focus_chains_;

  // Owns the FocusChainListener binding for |this|. Last member, so it is destroyed first; a
  // fidl::ServerBinding makes no calls into its server after destruction.
  std::optional<fidl::ServerBinding<fuf::FocusChainListener>> focus_chain_listener_binding_;
};

TEST_F(FlatlandFocusIntegrationTest, RequestValidity_RequestUnconnected_ShouldFail) {
  EXPECT_EQ(CountReceivedFocusChains(), 0u);

  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Not connected yet, so focus change requests should fail.
  EXPECT_FALSE(RequestFocusChange(*root_focuser_, child_view_ref));
  RunLoopWithTimeout(kWaitTime);
  EXPECT_EQ(CountReceivedFocusChains(), 0u);
}

TEST_F(FlatlandFocusIntegrationTest, RequestValidity_RequestConnected_ShouldSucceed) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Attach to root.
  AttachToRoot(std::move(parent_token));

  EXPECT_EQ(CountReceivedFocusChains(), 0u);
  // Move focus from the root to the child view.
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, child_view_ref));
  RunLoopUntil([this] { return CountReceivedFocusChains() == 1; });
  // FocusChain should contain root view + child view.
  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 2u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[0], root_view_ref_);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[1], child_view_ref);
}

TEST_F(FlatlandFocusIntegrationTest, RequestValidity_SelfRequest_ShouldSucceed) {
  // Set up the child view and attach it to the root.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  AttachToRoot(std::move(parent_token));

  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto [child_focuser_client_end, child_focuser_server_end] =
      fidl::CreateEndpoints<fuv::Focuser>().value();
  SimpleWatcherClient<fuv::Focuser> child_focuser(std::move(child_focuser_client_end),
                                                  dispatcher());
  ViewBoundProtocols protocols;
  protocols.view_focuser(std::move(child_focuser_server_end));
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Child is not focused. Trying to move focus at this point should fail.
  EXPECT_FALSE(RequestFocusChange(child_focuser, child_view_ref));
  EXPECT_EQ(CountReceivedFocusChains(), 0u);
  // First move focus from the root view to the child view.
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, child_view_ref));
  // Then move focus from the child view to itself. Should now succeed.
  EXPECT_TRUE(RequestFocusChange(child_focuser, child_view_ref));
  // Should only receive one focus chain, since it didn't change from the second request.
  RunLoopUntil([this] { return CountReceivedFocusChains() == 1; });
  RunLoopWithTimeout(kWaitTime);
  EXPECT_EQ(CountReceivedFocusChains(), 1u);
  // Should contain root view + child view.
  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 2u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[0], root_view_ref_);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[1], child_view_ref);
}

// Scene:
//   parent
//     |
//   child
//     |
// grandchild
//
// 1. Move focus to child.
// 2. Set auto focus from parent to grandchild.
// 3. Attempt to move focus back to parent.
// 4. Observe focus moving directly to grandchild.
TEST_F(FlatlandFocusIntegrationTest, AutoFocus_RequestFocus_Interaction) {
  // Set up the granchild view.
  auto [grandchild_token, middleparent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> grandchild_session;
  fuv::ViewRef grandchild_view_ref;
  {
    grandchild_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    grandchild_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    EXPECT_TRUE((*grandchild_session)
                    ->CreateView2(
                        {{.token = std::move(grandchild_token),
                          .view_identity = std::move(identity),
                          .protocols = {},
                          .parent_viewport_watcher =
                              fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value().server}})
                    .is_ok());
    BlockingPresent(this, *grandchild_session);
  }

  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  fuv::ViewRef child_view_ref;
  {
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    EXPECT_TRUE((*child_session)
                    ->CreateView2(
                        {{.token = std::move(child_token),
                          .view_identity = std::move(identity),
                          .protocols = {},
                          .parent_viewport_watcher =
                              fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value().server}})
                    .is_ok());

    // Attach grandchild to child.
    ViewportProperties properties;
    properties.logical_size(
        fuchsia_math::SizeU(kDefaultLogicalPixelSize, kDefaultLogicalPixelSize));
    const TransformId kTransform(1);
    const ContentId kContent(1);
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
    EXPECT_TRUE(
        (*child_session)
            ->CreateViewport({{.viewport_id = kContent,
                               .token = std::move(middleparent_token),
                               .properties = std::move(properties),
                               .child_view_watcher =
                                   fidl::CreateEndpoints<fuc::ChildViewWatcher>().value().server}})
            .is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
    EXPECT_TRUE((*child_session)
                    ->SetContent({{.transform_id = kTransform, .content_id = kContent}})
                    .is_ok());
    BlockingPresent(this, *child_session);
  }

  // Attach to root.
  AttachToRoot(std::move(parent_token));

  // Move focus from the root to the child view.
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, child_view_ref));
  RunLoopUntil([this] { return CountReceivedFocusChains() == 1; });
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value().back(), child_view_ref);

  // FocusChain should contain root view + child view.
  SetAutoFocus(*root_focuser_, grandchild_view_ref);
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, root_view_ref_));
  RunLoopUntil([this] { return CountReceivedFocusChains() == 2; });

  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 3u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[0], root_view_ref_);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[1], child_view_ref);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[2], grandchild_view_ref);
}

// Scene:
//   parent       parent        parent
//           ->     |      ->
//   child        child         child
//
// 1. Set parent's auto focus target to child.
// 2. Connect child to scene. Observe focus moving to child.
// 3. Disconnect child from scene. Observe focus return to parent.
TEST_F(FlatlandFocusIntegrationTest, AutoFocus_SceneUpdate_Interaction) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  fuv::ViewRef child_view_ref;
  {
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    EXPECT_TRUE((*child_session)
                    ->CreateView2(
                        {{.token = std::move(child_token),
                          .view_identity = std::move(identity),
                          .protocols = {},
                          .parent_viewport_watcher =
                              fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value().server}})
                    .is_ok());
    BlockingPresent(this, *child_session);
  }

  SetAutoFocus(*root_focuser_, child_view_ref);

  // Nothing should happen.
  RunLoopWithTimeout(zx::msec(1));
  EXPECT_EQ(CountReceivedFocusChains(), 0);

  // Attach to root.
  AttachToRoot(std::move(parent_token));

  // Auto focus should kick in.
  RunLoopUntil([this] { return CountReceivedFocusChains() == 1; });
  ASSERT_EQ(LastFocusChain()->focus_chain().value().size(), 2u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value().back(), child_view_ref);

  // Disconnect from root.
  EXPECT_TRUE((*root_session_)->SetRootTransform({{.transform_id = TransformId(0)}}).is_ok());
  BlockingPresent(this, *root_session_);

  // Observe focus returning to root.
  RunLoopUntil([this] { return CountReceivedFocusChains() == 2; });
  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 1u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value().back(), root_view_ref_);
}

// Scene:
//   parent
//     |
//   child (anonymous)
//     |
// grandchild
TEST_F(FlatlandFocusIntegrationTest, FocusRequest_ChildOfAnonymousView_ShouldFail) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [grandchild_token, grandchild_parent_token] = scenic::cpp::ViewCreationTokenPair::New();

  // Create the anonymous child view and attach the grandchild to it.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*child_session)
                    ->CreateView({{.token = std::move(child_token),
                                   .parent_viewport_watcher =
                                       std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    ViewportProperties properties;
    properties.logical_size(
        fuchsia_math::SizeU(kDefaultLogicalPixelSize, kDefaultLogicalPixelSize));
    const TransformId kRootTransform(1);
    const ContentId kRootContent(1);
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE(
        (*child_session)
            ->CreateViewport({{.viewport_id = kRootContent,
                               .token = std::move(grandchild_parent_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)
                    ->SetContent({{.transform_id = kRootTransform, .content_id = kRootContent}})
                    .is_ok());
    BlockingPresent(this, *child_session);
  }

  // Create the named grandchild view.
  std::unique_ptr<FlatlandClientWithEventHandler> grandchild_session;
  grandchild_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto grandchild_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*grandchild_session)
                    ->CreateView2({{.token = std::move(grandchild_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *grandchild_session);
  }

  AttachToRoot(std::move(parent_token));

  EXPECT_EQ(CountReceivedFocusChains(), 0u);
  // Attempt to move focus from the root to the grandchild view.
  EXPECT_FALSE(RequestFocusChange(*root_focuser_, grandchild_view_ref));
}

TEST_F(FlatlandFocusIntegrationTest, ChildView_CreatedBeforeAttachingToRoot_ShouldNotKillFocuser) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto [child_focuser_client_end, child_focuser_server_end] =
      fidl::CreateEndpoints<fuv::Focuser>().value();
  SimpleWatcherClient<fuv::Focuser> child_focuser(std::move(child_focuser_client_end),
                                                  dispatcher());

  ViewBoundProtocols protocols;
  protocols.view_focuser(std::move(child_focuser_server_end));
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Attach to root.
  AttachToRoot(std::move(parent_token));

  // The child_focuser should not die.
  RunLoopUntilIdle();
  EXPECT_TRUE(child_focuser.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest, FocusChain_Updated_OnViewDisconnect) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Attach to root.
  AttachToRoot(std::move(parent_token));

  EXPECT_EQ(CountReceivedFocusChains(), 0u);
  // Try to move focus to child. Should succeed.
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, child_view_ref));
  RunLoopUntil([this] { return CountReceivedFocusChains() == 1u; });  // Succeeds or times out.
  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 2u);

  // Disconnect the child and watch the focus chain update.
  const ContentId kRootContent(1);
  (*root_session_)->ReleaseViewport({{.viewport_id = kRootContent}}).Then([](auto&) {});
  BlockingPresent(this, *root_session_);
  RunLoopUntil([this] { return CountReceivedFocusChains() == 2u; });  // Succeeds or times out.
  EXPECT_EQ(LastFocusChain()->focus_chain().value().size(), 1u);
  EXPECT_VIEW_REF_MATCH(LastFocusChain()->focus_chain().value()[0], root_view_ref_);
}

TEST_F(FlatlandFocusIntegrationTest, ViewFocuserDisconnectDoesNotKillSession) {
  root_focuser_->Unbind();
  // Wait "long enough" and observe that the session channel doesn't close.
  RunLoopWithTimeout(kWaitTime);
}

TEST_F(FlatlandFocusIntegrationTest, ViewRefFocused_HappyCase) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  AttachToRoot(std::move(parent_token));
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto [child_focused_ptr_client_end, child_focused_ptr_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_focused_ptr(
      std::move(child_focused_ptr_client_end), dispatcher());
  ViewBoundProtocols protocols;
  protocols.view_ref_focused(std::move(child_focused_ptr_server_end));
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  // Watch for child focused event.
  std::optional<bool> child_focused;
  child_focused_ptr->Watch().Then(
      [&child_focused](fidl::Result<fuv::ViewRefFocused::Watch>& update) {
        ASSERT_TRUE(update.is_ok());
        ASSERT_TRUE(update->state().focused().has_value());
        child_focused = update->state().focused().value();
      });
  RunLoopUntilIdle();
  EXPECT_FALSE(child_focused.has_value());

  // Focus the child and confirm the event arriving.
  EXPECT_TRUE(RequestFocusChange(*root_focuser_, child_view_ref));
  RunLoopUntil([&child_focused] { return child_focused.has_value(); });
  EXPECT_TRUE(child_focused.value());
  EXPECT_TRUE(child_focused_ptr.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest,
       ChildView_PresentsBeforeParentPresent_ShouldNotKillVrfEndpoint) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto [child_focused_ptr_client_end, child_focused_ptr_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_focused_ptr(
      std::move(child_focused_ptr_client_end), dispatcher());

  ViewBoundProtocols protocols;
  protocols.view_ref_focused(std::move(child_focused_ptr_server_end));
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  // The child's Present call generates a new snapshot that includes the ViewRef.
  BlockingPresent(this, *child_session);

  // The parent view creates its Viewport later, and calls Present to commit.
  // The parent/child commit order should not matter.
  AttachToRoot(std::move(parent_token));

  // The child_focused_ptr should not die.
  RunLoopUntilIdle();
  EXPECT_TRUE(child_focused_ptr.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest,
       ChildView_PresentsAfterParentPresent_ShouldNotKillVrfEndpoint) {
  // Set up the child view.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto [child_focused_ptr_client_end, child_focused_ptr_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_focused_ptr(
      std::move(child_focused_ptr_client_end), dispatcher());

  ViewBoundProtocols protocols;
  protocols.view_ref_focused(std::move(child_focused_ptr_server_end));
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  // The parent acts first, which causes a snapshot to be generated *without* the child's ViewRef.
  // The child_focused_ptr should remain alive, because it is not yet bound.
  AttachToRoot(std::move(parent_token));

  BlockingPresent(this, *child_session);
  // The child_focused_ptr should not die.
  RunLoopUntilIdle();
  EXPECT_TRUE(child_focused_ptr.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest, ViewBoundChannels_ShouldSurviveViewDisconnect) {
  // Set up the child view and attach to root.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  auto [focused_client_end, focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> focused(std::move(focused_client_end), dispatcher());

  auto [focuser_client_end, focuser_server_end] = fidl::CreateEndpoints<fuv::Focuser>().value();
  SimpleWatcherClient<fuv::Focuser> focuser(std::move(focuser_client_end), dispatcher());

  ViewBoundProtocols protocols;
  protocols.view_ref_focused(std::move(focused_server_end));
  protocols.view_focuser(std::move(focuser_server_end));
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  AttachToRoot(std::move(parent_token));

  RunLoopUntilIdle();
  EXPECT_TRUE(focused.is_bound());
  EXPECT_TRUE(focuser.is_bound());

  // Disconnect from root and observe channels survive.
  EXPECT_TRUE((*root_session_)->SetRootTransform({{.transform_id = TransformId(0)}}).is_ok());
  BlockingPresent(this, *root_session_);
  RunLoopUntilIdle();
  EXPECT_TRUE(focused.is_bound());
  EXPECT_TRUE(focuser.is_bound());

  // Reconnect and observe that channels survive.
  EXPECT_TRUE((*root_session_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  BlockingPresent(this, *root_session_);
  RunLoopUntilIdle();
  EXPECT_TRUE(focused.is_bound());
  EXPECT_TRUE(focuser.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest, ViewRefFocusedDisconnectedWhenSessionDies) {
  // Set up the child view and attach to root.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  auto [focused_client_end, focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> focused(std::move(focused_client_end), dispatcher());

  ViewBoundProtocols protocols;
  protocols.view_ref_focused(std::move(focused_server_end));
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = scenic::cpp::NewViewIdentityOnCreation(),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *child_session);

  AttachToRoot(std::move(parent_token));

  RunLoopUntilIdle();
  EXPECT_TRUE(focused.is_bound());

  // Kill Child session.
  bool child_dead = false;
  child_session->set_on_error([&child_dead](auto& error) { child_dead = true; });

  // Invalid op since a transform ID of 0 is reserved.
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = TransformId(0)}}).is_ok());

  // Trigger session death.
  EXPECT_TRUE((*child_session)->Present({}).is_ok());
  RunLoopUntil([&child_dead, &child_session] { return child_dead || !child_session->is_bound(); });

  // Trigger a new snapshot to be published.
  BlockingPresent(this, *root_session_);

  RunLoopUntil([&focused] { return !focused.is_bound(); });  // Succeeds or times out.
  EXPECT_FALSE(focused.is_bound());
}

TEST_F(FlatlandFocusIntegrationTest, ViewRefFocusedDisconnectDoesNotKillSession) {
  root_focused_->Unbind();

  // Observe that the channel doesn't close after a blocking present.
  BlockingPresent(this, *root_session_);
}

#undef EXPECT_VIEW_REF_MATCH

}  // namespace integration_tests
