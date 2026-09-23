// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <lib/zx/time.h>
#include <zircon/status.h>

#include <memory>
#include <optional>
#include <utility>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"

// This test exercises the fuchsia.ui.views.ViewRefInstalled protocol implemented by Scenic
// in the context of the Flatland compositor interface.
namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuv = fuchsia_ui_views;

// Test fixture that sets up an environment with a Scenic we can connect to.
class FlatlandViewRefInstalledIntegrationTest : public ScenicCtfTest {
 protected:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    // Set up root view.
    root_session_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_session_->set_on_close(FailOnClose("Lost connection to Scenic"));

    fuc::ViewBoundProtocols protocols;
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    EXPECT_TRUE((*root_session_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
        std::move(parent_viewport_watcher_client_end), dispatcher());
    std::optional<fuc::LayoutInfo> layout_info;
    parent_viewport_watcher->GetLayout().Then(
        [&layout_info](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
          if (result.is_ok()) {
            layout_info = std::move(result->info());
          }
        });
    BlockingPresent(this, *root_session_);
    RunLoopUntil([&layout_info] { return layout_info.has_value(); });
    ASSERT_TRUE(layout_info->logical_size().has_value());
    display_width_ = layout_info->logical_size()->width();
    display_height_ = layout_info->logical_size()->height();

    view_ref_installed_ptr_ = fidl::Client<fuv::ViewRefInstalled>(
        ConnectIntoRealm<fuv::ViewRefInstalled>(), dispatcher());
  }

  // Create a new transform and viewport, then call |BlockingPresent| to wait for it to take
  // effect. This can be called only once per Flatland instance, because it uses hard-coded IDs for
  // the transform and viewport.
  void ConnectChildView(FlatlandClientWithEventHandler& flatland,
                        fuv::ViewportCreationToken&& token) {
    // Let the client_end die.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    fuc::ViewportProperties properties;
    properties.logical_size(fuchsia_math::SizeU(display_width_, display_height_));

    const fuc::TransformId kTransform(1);
    EXPECT_TRUE(flatland->CreateTransform({{.transform_id = kTransform}}).is_ok());
    EXPECT_TRUE(flatland->SetRootTransform({{.transform_id = kTransform}}).is_ok());

    const fuc::ContentId kContent(1);
    EXPECT_TRUE(
        flatland
            ->CreateViewport({{.viewport_id = kContent,
                               .token = std::move(token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE(
        flatland->SetContent({{.transform_id = kTransform, .content_id = kContent}}).is_ok());

    BlockingPresent(this, flatland);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> root_session_;
  fidl::Client<fuv::ViewRefInstalled> view_ref_installed_ptr_;

 private:
  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
};

TEST_F(FlatlandViewRefInstalledIntegrationTest, InvalidatedViewRef_ShouldReturnError) {
  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  {
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    view_ref_installed_ptr_->Watch({{.view_ref = std::move(identity.view_ref())}})
        .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
          result.emplace(std::move(watch_result));
        });
    RunLoopUntilIdle();
    EXPECT_FALSE(result.has_value());
  }  // |identity| goes out of scope. This will invalidate the ViewRef.

  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_error());
}

// The test exercises a two node topology:-
//  root_view
//      |
//  child_view
// |Watch()| on the child viewRef should return as soon as the child view gets connected to the root
// view.
TEST_F(FlatlandViewRefInstalledIntegrationTest, InstalledViewRef_ShouldReturnImmediately) {
  // Create the child view and connect it to the root view.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fuc::ViewBoundProtocols protocols;
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto view_ref_clone = scenic::cpp::CloneViewRef(identity.view_ref());
  ConnectChildView(*root_session_, std::move(parent_token));

  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  BlockingPresent(this, *child_session);

  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  view_ref_installed_ptr_->Watch({{.view_ref = std::move(view_ref_clone)}})
      .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
        result.emplace(std::move(watch_result));
      });

  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_ok());
}

// The test exercises a two node topology:-
//  root_view
//      |
//  child_view
// |Watch()| on the child viewRef should only return when a child view gets connected to the root
// view.
TEST_F(FlatlandViewRefInstalledIntegrationTest, WaitedOnViewRef_ShouldReturnWhenInstalled) {
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto view_ref_clone = scenic::cpp::CloneViewRef(identity.view_ref());

  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  view_ref_installed_ptr_->Watch({{.view_ref = std::move(view_ref_clone)}})
      .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
        result.emplace(std::move(watch_result));
      });

  // ViewRef not installed; should not return yet.
  RunLoopUntilIdle();
  EXPECT_FALSE(result.has_value());

  // Create the child view with the viewRef.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  child_session->set_on_error([](fidl::Event<fuc::Flatland::OnError>& event) {
    // Don't be silent about errors.
    FX_LOGS(ERROR) << "Child session failed with: " << fidl::ToUnderlying(event.error());
    FAIL();
  });

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fuc::ViewBoundProtocols protocols;

  ConnectChildView(*root_session_, std::move(parent_token));

  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  BlockingPresent(this, *child_session);

  // |Watch()| returns as the view ref is now installed.
  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_ok());
}

// The view tree topology changes in the following manner in this test:-
//  root view     root_view       root view
//             ->     |       ->
//               child_view       child view
// |Watch()| on the child viewRef will always return once the view gets connected to the root view
// provided that the view is not destroyed.
TEST_F(FlatlandViewRefInstalledIntegrationTest,
       InstalledAndDisconnectedViewRef_ShouldReturnResponse) {
  // Create the child view and connect it to the root view.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fuc::ViewBoundProtocols protocols;
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto view_ref_clone = scenic::cpp::CloneViewRef(identity.view_ref());
  ConnectChildView(*root_session_, std::move(parent_token));

  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  BlockingPresent(this, *child_session);

  // Disconnect the child view.
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = fuc::TransformId(0)}}).is_ok());
  BlockingPresent(this, *child_session);

  // Watch should still return true, since the view has been previously installed.
  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  view_ref_installed_ptr_->Watch({{.view_ref = std::move(view_ref_clone)}})
      .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
        result.emplace(std::move(watch_result));
      });
  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_ok());
}

// The view tree topology changes in the following manner in this test:-
//  root view     root_view       root view
//             ->     |       ->
//               child_view
// |Watch()| on the child viewRef will return an error since the child view is released.
TEST_F(FlatlandViewRefInstalledIntegrationTest, InstalledAndDestroyedViewRef_ShouldReturnError) {
  // Create the child view and connect it to the root view.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fuc::ViewBoundProtocols protocols;
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto view_ref_clone = scenic::cpp::CloneViewRef(identity.view_ref());
  ConnectChildView(*root_session_, std::move(parent_token));

  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  BlockingPresent(this, *child_session);

  // Release the child view.
  EXPECT_TRUE((*child_session)->ReleaseView().is_ok());
  BlockingPresent(this, *child_session);

  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  view_ref_installed_ptr_->Watch({{.view_ref = std::move(view_ref_clone)}})
      .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
        result.emplace(std::move(watch_result));
      });
  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_error());
}

// The test exercises a three node topology:-
//  root_view
//      |
//  parent_view
//      |
//   child view
// |Watch()| on the child viewRef returns only when the child view gets connected to the root of the
// graph transitively.
TEST_F(FlatlandViewRefInstalledIntegrationTest, TransitiveConnection_ShouldReturnResponse) {
  // Create the parent view and connect it to the root view.
  auto parent_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  auto [parent_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(parent_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    BlockingPresent(this, *parent_session);
  }

  // Create the child view and connect it to the parent view.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  fuv::ViewRef child_view_ref;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ConnectChildView(*parent_session, std::move(parent_token));

    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    BlockingPresent(this, *child_session);
  }

  std::optional<fidl::Result<fuv::ViewRefInstalled::Watch>> result;
  view_ref_installed_ptr_->Watch({{.view_ref = std::move(child_view_ref)}})
      .Then([&result](fidl::Result<fuv::ViewRefInstalled::Watch>& watch_result) {
        result.emplace(std::move(watch_result));
      });
  // child view ref not installed; should not return yet.
  RunLoopUntilIdle();
  EXPECT_FALSE(result.has_value());

  // Now attach the whole thing to the root and observe that the child view ref is installed.
  ConnectChildView(*root_session_, std::move(parent_viewport_token));
  RunLoopUntil([&result] { return result.has_value(); });  // Succeeds or times out.
  EXPECT_TRUE(result->is_ok());
}

}  // namespace integration_tests
