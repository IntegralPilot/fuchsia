// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <lib/zx/time.h>
#include <zircon/status.h>

#include <optional>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"

// This test exercises a two node topology and tests the signals propagated between the
// parent instance and the child instance.
namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuv = fuchsia_ui_views;

const fuc::TransformId kTransformId(1);
const fuc::ContentId kContentId(1);

// Test fixture that sets up an environment with a Scenic we can connect to.
class FlatlandViewIntegrationTest : public ScenicCtfTest {
 protected:
  void SetUp() override {
    ScenicCtfTest::SetUp();

    // Get the display's width and height.
    fidl::SyncClient singleton_display = ConnectSyncIntoRealm<fuchsia_ui_display_singleton::Info>();
    auto result = singleton_display->GetMetrics();
    ASSERT_TRUE(result.is_ok());

    display_width_ = result->info().extent_in_px()->width();
    display_height_ = result->info().extent_in_px()->height();
  }

  // Create a new transform and viewport, then call |BlockingPresent| to wait for it to take
  // effect. This can be called only once per Flatland instance, because it uses hard-coded IDs for
  // the transform and viewport.
  void CreateAndSetViewport(FlatlandClientWithEventHandler& flatland,
                            fuv::ViewportCreationToken&& viewport_creation_token,
                            fidl::ServerEnd<fuc::ChildViewWatcher> child_view_watcher_server_end) {
    fuc::ViewportProperties properties;
    properties.logical_size({{display_width_, display_height_}});

    ASSERT_TRUE(flatland->CreateTransform(kTransformId).is_ok());
    ASSERT_TRUE(flatland->SetRootTransform(kTransformId).is_ok());

    ASSERT_TRUE(
        flatland
            ->CreateViewport({{.viewport_id = kContentId,
                               .token = std::move(viewport_creation_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    ASSERT_TRUE(
        flatland->SetContent({{.transform_id = kTransformId, .content_id = kContentId}}).is_ok());

    BlockingPresent(this, flatland);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> MakeFlatland() {
    auto flatland = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    flatland->set_on_error([this](FlatlandClientWithEventHandler::OnErrorEvent& event) {
      // Log at INFO so that tests which deliberately induce errors don't require
      // `max_severity_logs` to be adjusted.
      FX_LOGS(INFO) << "Received FlatlandError " << static_cast<uint32_t>(event.error());
      last_error_ = event.error();
    });
    return flatland;
  }

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
  std::optional<fuc::FlatlandError> last_error_;
};

TEST_F(FlatlandViewIntegrationTest, ParentViewportWatcherUnbindsOnParentDeath) {
  std::unique_ptr<FlatlandClientWithEventHandler> child;
  auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(
      std::move(parent_viewport_watcher_client_end), dispatcher());
  // Create the child view.
  {
    child = MakeFlatland();

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*child)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *child);
  }

  // Create the parent view and connect the child view to it.
  {
    auto parent = MakeFlatland();
    auto [parent_view_token, display_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

    // Connect the parent view to the display.
    SetFlatlandDisplayContent(std::move(display_viewport_token));

    auto [display_viewport_watcher_client_end, display_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*parent)
                    ->CreateView2({{.token = std::move(parent_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(display_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *parent);

    // Connect the child view to the parent view.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    CreateAndSetViewport(*parent, std::move(parent_viewport_token),
                         std::move(child_view_watcher_server_end));

    EXPECT_TRUE(parent_viewport_watcher.is_bound());
  }

  // The parent instance goes out of scope and dies. Wait for a frame to guarantee parent's death.
  BlockingPresent(this, *child);
  EXPECT_TRUE(child->is_valid());

  // The ParentViewportWatcher unbinds as the parent died.
  EXPECT_FALSE(parent_viewport_watcher.is_bound());
}

TEST_F(FlatlandViewIntegrationTest, ParentViewportWatcherUnbindsOnInvalidTokenTest) {
  // Create the flatland view.
  auto flatland = MakeFlatland();
  fuv::ViewCreationToken invalid_token;

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(
      std::move(parent_viewport_watcher_client_end), dispatcher());
  auto identity = scenic::cpp::NewViewIdentityOnCreation();

  // Use an invalid ViewCreationToken in |CreateView2|.
  auto result = (*flatland)->CreateView2(
      {{.token = std::move(invalid_token),
        .view_identity = std::move(identity),
        .protocols = {},
        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}});
  (void)result;
  RunLoopUntilIdle();

  // The ParentViewportWatcher unbinds as we supply an invalid ViewCreationToken.
  EXPECT_FALSE(parent_viewport_watcher.is_bound());
}

TEST_F(FlatlandViewIntegrationTest, ParentViewportWatcherUnbindsOnReleaseView) {
  // Create the parent view.
  auto parent = MakeFlatland();
  auto [parent_view_creation_token, display_viewport_token] =
      scenic::cpp::ViewCreationTokenPair::New();

  // Connect the parent view to the display.
  SetFlatlandDisplayContent(std::move(display_viewport_token));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  SimpleWatcherClient<fuc::ParentViewportWatcher> parent_viewport_watcher(
      std::move(parent_viewport_watcher_client_end), dispatcher());
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  ASSERT_TRUE((*parent)
                  ->CreateView2(
                      {{.token = std::move(parent_view_creation_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());

  // Since there is no Present in FlatlandDisplay, receiving this callback ensures that all
  // FlatlandDisplay calls are processed.
  bool connected = false;
  parent_viewport_watcher->GetLayout().Then([&connected](auto& result) {
    if (result.is_ok()) {
      connected = true;
    }
  });
  RunLoopUntil([&connected] { return connected; });
  BlockingPresent(this, *parent);

  EXPECT_TRUE(parent_viewport_watcher.is_bound());

  // Disconnect the parent view from the root.
  ASSERT_TRUE((*parent)->ReleaseView().is_ok());
  BlockingPresent(this, *parent);

  // The ParentViewportWatcher unbinds as the parent view is now disconnected.
  EXPECT_FALSE(parent_viewport_watcher.is_bound());
}

TEST_F(FlatlandViewIntegrationTest, ChildViewWatcherUnbindsOnChildDeath) {
  auto parent = MakeFlatland();

  // Create the parent view and connect it to the display.
  {
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_viewport_token));

    auto [display_viewport_watcher_client_end, display_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*parent)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(display_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *parent);
  }

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
  SimpleWatcherClient<fuc::ChildViewWatcher> child_view_watcher(
      std::move(child_view_watcher_client_end), dispatcher());

  // Create the child view and connect it to the parent view.
  {
    auto child = MakeFlatland();
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*child)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *child);

    CreateAndSetViewport(*parent, std::move(parent_viewport_token),
                         std::move(child_view_watcher_server_end));

    EXPECT_TRUE(child_view_watcher.is_bound());
  }

  // The child instance dies as it goes out of scope. Wait for a frame to guarantee child's death.
  BlockingPresent(this, *parent);

  // The ChildViewWatcher unbinds as the child instance died.
  EXPECT_FALSE(child_view_watcher.is_bound());
}

TEST_F(FlatlandViewIntegrationTest, ChildViewWatcherUnbindsOnInvalidToken) {
  // Create the parent view.
  auto parent = MakeFlatland();

  auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

  // Connect the parent view to the display.
  SetFlatlandDisplayContent(std::move(parent_viewport_token));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  ASSERT_TRUE((*parent)
                  ->CreateView2(
                      {{.token = std::move(child_view_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *parent);

  fuv::ViewportCreationToken invalid_token;
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
  SimpleWatcherClient<fuc::ChildViewWatcher> child_view_watcher(
      std::move(child_view_watcher_client_end), dispatcher());

  // Create a viewport using an invalid token.
  fuc::ViewportProperties properties;
  properties.logical_size({{display_width_, display_height_}});

  ASSERT_TRUE((*parent)->CreateTransform(kTransformId).is_ok());
  ASSERT_TRUE((*parent)->SetRootTransform(kTransformId).is_ok());

  auto result =
      (*parent)->CreateViewport({{.viewport_id = kContentId,
                                  .token = std::move(invalid_token),
                                  .properties = std::move(properties),
                                  .child_view_watcher = std::move(child_view_watcher_server_end)}});
  (void)result;
  ASSERT_TRUE(
      (*parent)->SetContent({{.transform_id = kTransformId, .content_id = kContentId}}).is_ok());

  RunLoopUntilIdle();

  // ChildViewWatcher unbinds as an invalid token was supplied to |CreateViewport|.
  EXPECT_FALSE(child_view_watcher.is_bound());
}

// This test checks whether the |CONNECTED_TO_DISPLAY| and |DISCONNECTED_FROM_DISPLAY| signals are
// propagated correctly.
TEST_F(FlatlandViewIntegrationTest, ParentViewportStatusTest) {
  auto parent = MakeFlatland();
  // Create the parent view and connect it to the display.
  {
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_viewport_token));

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*parent)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *parent);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> child;
  std::optional<fuc::ParentViewportStatus> parent_status;
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
      std::move(parent_viewport_watcher_client_end), dispatcher());
  // Create the child view and connect it to the parent.
  {
    child = MakeFlatland();
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*child)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    parent_viewport_watcher->GetStatus().Then(
        [&parent_status](fidl::Result<fuc::ParentViewportWatcher::GetStatus>& result) {
          if (result.is_ok()) {
            parent_status = result.value().status();
          }
        });

    BlockingPresent(this, *child);

    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    CreateAndSetViewport(*parent, std::move(parent_viewport_token),
                         std::move(child_view_watcher_server_end));
  }

  // The child instance gets a |CONNECTED_TO_DISPLAY| signal when the child view is connected to the
  // root and when both the parent and the child call |Present|.
  RunLoopUntil([&parent_status] { return parent_status.has_value(); });
  ASSERT_TRUE(parent_status.has_value());
  EXPECT_EQ(parent_status.value(), fuc::ParentViewportStatus::kConnectedToDisplay);
  parent_status.reset();

  // Disconnect the child view.
  ASSERT_TRUE((*parent)
                  ->SetContent({{.transform_id = kTransformId, .content_id = fuc::ContentId(0)}})
                  .is_ok());
  parent_viewport_watcher->GetStatus().Then(
      [&parent_status](fidl::Result<fuc::ParentViewportWatcher::GetStatus>& result) {
        if (result.is_ok()) {
          parent_status = result.value().status();
        }
      });

  BlockingPresent(this, *parent);

  // The child view gets the |DISCONNECTED_FROM_DISPLAY| signal as it was disconnected from its
  // parent.
  RunLoopUntil([&parent_status] { return parent_status.has_value(); });
  ASSERT_TRUE(parent_status.has_value());
  EXPECT_EQ(parent_status.value(), fuc::ParentViewportStatus::kDisconnectedFromDisplay);
}

// This test checks whether the |CONTENT_HAS_PRESENTED| signal propagates correctly.
TEST_F(FlatlandViewIntegrationTest, ChildViewStatusTest) {
  auto parent = MakeFlatland();
  // Create the parent view and connect it to the display.
  {
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_viewport_token));

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*parent)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *parent);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> child;
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
  fidl::Client<fuc::ChildViewWatcher> child_view_watcher(std::move(child_view_watcher_client_end),
                                                         dispatcher());
  std::optional<fuc::ChildViewStatus> child_status;
  // Create the child view and connect it to the parent view.
  {
    child = MakeFlatland();
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*child)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    CreateAndSetViewport(*parent, std::move(parent_viewport_token),
                         std::move(child_view_watcher_server_end));

    child_view_watcher->GetStatus().Then(
        [&child_status](fidl::Result<fuc::ChildViewWatcher::GetStatus>& result) {
          if (result.is_ok()) {
            child_status = result.value().status();
          }
        });

    BlockingPresent(this, *child);
  }

  // The parent instance gets the |CONTENT_HAS_PRESENTED| signal when the child view calls
  // |Present|.
  RunLoopUntil([&child_status] { return child_status.has_value(); });
  ASSERT_TRUE(child_status.has_value());
  EXPECT_EQ(child_status.value(), fuc::ChildViewStatus::kContentHasPresented);
}

TEST_F(FlatlandViewIntegrationTest, GetViewRefTest) {
  auto parent = MakeFlatland();
  auto [parent_view_creation_token, display_viewport_token] =
      scenic::cpp::ViewCreationTokenPair::New();

  // Create the parent view.
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    ASSERT_TRUE((*parent)
                    ->CreateView2({{.token = std::move(parent_view_creation_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    BlockingPresent(this, *parent);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> child;
  std::optional<fuc::ChildViewStatus> child_status;
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
  fidl::Client<fuc::ChildViewWatcher> child_view_watcher(std::move(child_view_watcher_client_end),
                                                         dispatcher());
  std::optional<fuv::ViewRef> child_view_ref;
  fuv::ViewRef expected_child_view_ref;

  // Create the child view and connect it to the parent view.
  {
    child = MakeFlatland();
    auto [child_view_token, parent_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    expected_child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ASSERT_TRUE((*child)
                    ->CreateView2({{.token = std::move(child_view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    CreateAndSetViewport(*parent, std::move(parent_viewport_token),
                         std::move(child_view_watcher_server_end));

    child_view_watcher->GetStatus().Then(
        [&child_status](fidl::Result<fuc::ChildViewWatcher::GetStatus>& result) {
          if (result.is_ok()) {
            child_status = result.value().status();
          }
        });

    child_view_watcher->GetViewRef().Then(
        [&child_view_ref](fidl::Result<fuc::ChildViewWatcher::GetViewRef>& result) {
          if (result.is_ok()) {
            child_view_ref = std::move(result.value().view_ref());
          }
        });

    BlockingPresent(this, *child);
  }

  // The parent instance gets the |CONTENT_HAS_PRESENTED| signal when the child view calls
  // |Present|.
  RunLoopUntil([&child_status] { return child_status.has_value(); });
  ASSERT_TRUE(child_status.has_value());
  EXPECT_EQ(child_status.value(), fuc::ChildViewStatus::kContentHasPresented);

  // Note that although CONTENT_HAS_PRESENTED is signaled, GetViewRef() does not yet return the ref.
  // This is because although the parent and child are connected, neither appears in the global
  // topology, because neither is connected to the root.
  EXPECT_FALSE(child_view_ref.has_value());

  SetFlatlandDisplayContent(std::move(display_viewport_token));

  // Parent's ChildViewWatcher receives the view ref as it is now connected to the display.
  RunLoopUntil([&child_view_ref] { return child_view_ref.has_value(); });
  EXPECT_EQ(ExtractKoid(child_view_ref->reference()),
            ExtractKoid(expected_child_view_ref.reference()));
}

TEST_F(FlatlandViewIntegrationTest, SpuriousReleaseViewYieldsError) {
  auto flatland = MakeFlatland();
  ASSERT_TRUE((*flatland)->ReleaseView().is_ok());
  ASSERT_TRUE((*flatland)->Present({}).is_ok());
  RunLoopUntil([this] { return last_error_.has_value(); });
  EXPECT_EQ(last_error_, fuc::FlatlandError::kBadOperation);
}

TEST_F(FlatlandViewIntegrationTest, DevicePixelRatioUpdatesCorrectlyEvenWithNoSnapshotChanges) {
  auto parent = MakeFlatland();
  auto [parent_view_token, display_viewport_token] = scenic::cpp::ViewCreationTokenPair::New();

  // Connect the parent view to the display.
  SetFlatlandDisplayContent(std::move(display_viewport_token));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
      std::move(parent_viewport_watcher_client_end), dispatcher());
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  ASSERT_TRUE((*parent)
                  ->CreateView2(
                      {{.token = std::move(parent_view_token),
                        .view_identity = std::move(identity),
                        .protocols = {},
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  BlockingPresent(this, *parent);

  // Get the display's initial layout (DPR). Since there is no Present in FlatlandDisplay, receiving
  // this callback ensures that all previous FlatlandDisplay setup is fully processed.
  std::optional<fuc::LayoutInfo> initial_layout;
  parent_viewport_watcher->GetLayout().Then(
      [&initial_layout](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
        if (result.is_ok()) {
          initial_layout = std::move(result.value().info());
        }
      });
  RunLoopUntil([&initial_layout] { return initial_layout.has_value(); });

  // Flush any pending snapshot changes (such as link resolution) so that subsequent
  // presents have no snapshot changes.
  BlockingPresent(this, *parent);

  const float initial_dpr_x = initial_layout->device_pixel_ratio()->x();
  const float initial_dpr_y = initial_layout->device_pixel_ratio()->y();

  // Register a second GetLayout() call. This call MUST hang because no properties have changed.
  std::optional<fuc::LayoutInfo> updated_layout;
  parent_viewport_watcher->GetLayout().Then(
      [&updated_layout](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
        if (result.is_ok()) {
          updated_layout = std::move(result.value().info());
        }
      });
  RunLoopUntilIdle();
  EXPECT_FALSE(updated_layout.has_value());

  // Set a new DPR on the display.
  const float kNewDprX = initial_dpr_x + 0.5f;
  const float kNewDprY = initial_dpr_y + 0.5f;
  fuchsia_math::VecF dpr{{.x = kNewDprX, .y = kNewDprY}};
  SetFlatlandDisplayDevicePixelRatio(std::move(dpr));

  // The hanging get should now complete.
  RunLoopUntil([&updated_layout] { return updated_layout.has_value(); });

  ASSERT_TRUE(updated_layout.has_value());
  EXPECT_EQ(updated_layout->device_pixel_ratio()->x(), kNewDprX);
  EXPECT_EQ(updated_layout->device_pixel_ratio()->y(), kNewDprY);
}

}  // namespace integration_tests
