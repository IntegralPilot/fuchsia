// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.focus/cpp/fidl.h>
#include <fidl/fuchsia.ui.observation.geometry/cpp/fidl.h>
#include <fidl/fuchsia.ui.observation.scope/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <zircon/status.h>

#include <vector>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"

// This test exercises the fuchsia.ui.observation.scope.Registry protocol implemented by Scenic.

// TODO(https://fxbug.dev/42056965): This test duplicate lots of code in
// observer_registry_integration_test.cc. We should factor the shard pieces into
// a base test class and/or util library.

namespace {

using integration_tests::BlockingPresent;
using ExpectedLayout = std::pair<float, float>;

// Stores information about a view node present in a fuog_ViewDescriptor. Used for assertions.
struct SnapshotViewNode {
  std::optional<zx_koid_t> view_ref_koid;
  std::vector<uint32_t> children;
  std::optional<ExpectedLayout> layout;
};

// A helper class for creating a SnapshotViewNode vector.
class ViewBuilder {
 public:
  static ViewBuilder New() { return ViewBuilder(); }

  ViewBuilder& AddView(std::optional<zx_koid_t> view_ref_koid, std::vector<zx_koid_t> children,
                       std::optional<ExpectedLayout> layout = std::nullopt) {
    std::vector<uint32_t> view_node_children;
    for (auto child : children) {
      view_node_children.push_back(static_cast<uint32_t>(child));
    }

    SnapshotViewNode view_node = {.view_ref_koid = view_ref_koid,
                                  .children = std::move(view_node_children),
                                  .layout = layout};
    snapshot_view_nodes_.push_back(std::move(view_node));
    return *this;
  }

  std::vector<SnapshotViewNode> Build() { return snapshot_view_nodes_; }

 private:
  std::vector<SnapshotViewNode> snapshot_view_nodes_;
};

}  // namespace

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fuf = fuchsia_ui_focus;
namespace fuog = fuchsia_ui_observation_geometry;
namespace fuos = fuchsia_ui_observation_scope;
namespace fuv = fuchsia_ui_views;

struct DisplayDimensions {
  float width = 0.f, height = 0.f;
};

void AssertViewDescriptor(const fuog::ViewDescriptor& view_descriptor,
                          const SnapshotViewNode& expected_view_descriptor) {
  if (expected_view_descriptor.view_ref_koid.has_value()) {
    ASSERT_TRUE(view_descriptor.view_ref_koid().has_value());
    EXPECT_EQ(view_descriptor.view_ref_koid().value(),
              expected_view_descriptor.view_ref_koid.value());
  }

  ASSERT_TRUE(view_descriptor.children().has_value());
  ASSERT_EQ(view_descriptor.children()->size(), expected_view_descriptor.children.size());
  for (uint32_t i = 0; i < view_descriptor.children()->size(); i++) {
    EXPECT_EQ(view_descriptor.children()->at(i), expected_view_descriptor.children[i]);
  }

  if (expected_view_descriptor.layout.has_value()) {
    ASSERT_TRUE(view_descriptor.layout().has_value());
    auto& layout = view_descriptor.layout().value();

    EXPECT_TRUE(CmpFloatingValues(layout.extent().min().x(), 0.f));
    EXPECT_TRUE(CmpFloatingValues(layout.extent().min().y(), 0.f));
    EXPECT_TRUE(
        CmpFloatingValues(layout.extent().max().x(), expected_view_descriptor.layout->first));
    EXPECT_TRUE(
        CmpFloatingValues(layout.extent().max().y(), expected_view_descriptor.layout->second));
    EXPECT_TRUE(CmpFloatingValues(layout.pixel_scale().at(0), 1.f));
    EXPECT_TRUE(CmpFloatingValues(layout.pixel_scale().at(1), 1.f));
  }
}

void AssertViewTreeSnapshot(const fuog::ViewTreeSnapshot& snapshot,
                            std::vector<SnapshotViewNode> expected_snapshot_nodes) {
  ASSERT_TRUE(snapshot.views().has_value());
  ASSERT_EQ(snapshot.views()->size(), expected_snapshot_nodes.size());

  for (uint32_t i = 0; i < snapshot.views()->size(); i++) {
    AssertViewDescriptor(snapshot.views()->at(i), expected_snapshot_nodes[i]);
  }
}

bool CheckViewExistsInSnapshot(const fuog::ViewTreeSnapshot& snapshot, zx_koid_t view_ref_koid) {
  if (!snapshot.views().has_value()) {
    return false;
  }
  auto it = std::find_if(
      snapshot.views()->begin(), snapshot.views()->end(), [view_ref_koid](const auto& view) {
        return view.view_ref_koid().has_value() && view.view_ref_koid().value() == view_ref_koid;
      });
  return it != snapshot.views()->end();
}

// Returns the iterator to the first fuog_ViewTreeSnapshot in |updates| having |view_ref_koid|
// present.
std::vector<fuog::ViewTreeSnapshot>::const_iterator GetFirstSnapshotWithView(
    const std::vector<fuog::ViewTreeSnapshot>& updates, zx_koid_t view_ref_koid) {
  return std::find_if(updates.begin(), updates.end(), [view_ref_koid](auto& snapshot) {
    return CheckViewExistsInSnapshot(snapshot, view_ref_koid);
  });
}

// Test fixture that sets up an environment with Registry protocol we can connect to. This test
// fixture is used for tests where the view nodes are created by Flatland instances.
class FlatlandObserverRegistryIntegrationTest : public ScenicCtfTest,
                                                public fidl::Server<fuf::FocusChainListener> {
 protected:
  FlatlandObserverRegistryIntegrationTest() = default;

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

    scoped_observer_registry_client_.Bind(ConnectIntoRealm<fuos::Registry>(), dispatcher());

    // Set up root view.
    root_session_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_session_->set_on_close(FailOnClose("Lost connection to Scenic"));

    fuc::ViewBoundProtocols protocols;
    auto [root_focuser_client_end, root_focuser_server_end] =
        fidl::CreateEndpoints<fuv::Focuser>().value();
    root_focuser_.Bind(std::move(root_focuser_client_end), dispatcher());
    protocols.view_focuser(std::move(root_focuser_server_end));

    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
        std::move(parent_viewport_watcher_client_end), dispatcher());

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    root_view_ref_koid_ = ExtractKoid(identity.view_ref());
    EXPECT_TRUE((*root_session_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

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
    display_width_ = static_cast<float>(layout_info->logical_size()->width());
    display_height_ = static_cast<float>(layout_info->logical_size()->height());

    // Now that the scene exists, wait for a valid focus chain and for the display size.
    RunLoopUntil([this] {
      return CountReceivedFocusChains() >= 1u && display_width_ != 0 && display_height_ != 0;
    });
    EXPECT_TRUE(LastFocusChain()->focus_chain().has_value());
    ASSERT_EQ(LastFocusChain()->focus_chain()->size(), 1u);

    observed_focus_chains_.clear();
  }

  // Create a new transform and viewport, then call |BlockingPresent| to wait for it to take
  // effect. This can be called only once per Flatland instance, because it uses hard-coded IDs for
  // the transform and viewport.
  void ConnectChildView(FlatlandClientWithEventHandler& flatland,
                        fuv::ViewportCreationToken&& token) {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    fuc::ViewportProperties properties;
    properties.logical_size(fuchsia_math::SizeU{{.width = kDefaultSize, .height = kDefaultSize}});

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

  // |fuchsia_ui_focus::FocusChainListener|
  void OnFocusChange(OnFocusChangeRequest& request,
                     OnFocusChangeCompleter::Sync& completer) override {
    observed_focus_chains_.push_back(std::move(request.focus_chain()));
    completer.Reply();
  }

  size_t CountReceivedFocusChains() const { return observed_focus_chains_.size(); }

  const fuf::FocusChain* LastFocusChain() const {
    if (observed_focus_chains_.empty()) {
      return nullptr;
    } else {
      return &observed_focus_chains_.back();
    }
  }

  const uint32_t kDefaultSize = 1;
  float display_width_ = 0;
  float display_height_ = 0;
  fidl::Client<fuos::Registry> scoped_observer_registry_client_;
  std::unique_ptr<FlatlandClientWithEventHandler> root_session_;
  zx_koid_t root_view_ref_koid_ = ZX_KOID_INVALID;
  fidl::Client<fuv::Focuser> root_focuser_;

 private:
  std::vector<fuf::FocusChain> observed_focus_chains_;

  // Owns the FocusChainListener binding for |this|. Last member, so it is destroyed first; a
  // fidl::ServerBinding makes no calls into its server after destruction.
  std::optional<fidl::ServerBinding<fuf::FocusChainListener>> focus_chain_listener_binding_;
};

// The client should receive updates whenever there is a change in the topology of the view tree.
// The view tree topology changes in the following manner in this test:
// root_view -> root_view    ->   root_view   ->  root_view
//                  |                 |               |
//            parent_view       parent_view     parent_view
//                                    |
//                               child_view
TEST_F(FlatlandObserverRegistryIntegrationTest, ClientReceivesTopologyUpdatesForFlatland) {
  auto [view_tree_watcher_client_end, view_tree_watcher_server_end] =
      fidl::CreateEndpoints<fuog::ViewTreeWatcher>().value();
  fidl::Client<fuog::ViewTreeWatcher> view_tree_watcher(std::move(view_tree_watcher_client_end),
                                                        dispatcher());

  // Set up the parent_view and connect it to the root_view.
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  zx_koid_t parent_view_ref_koid = ZX_KOID_INVALID;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    parent_view_ref_koid = ExtractKoid(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token));

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    // Register view tree watcher before proceeding.
    // We can't register earlier, because we need to know `parent_view_ref_koid` to register an
    // observer scoped to the parent view.
    std::optional<bool> result;
    scoped_observer_registry_client_
        ->RegisterScopedViewTreeWatcher({{.context_view = parent_view_ref_koid,
                                          .watcher = std::move(view_tree_watcher_server_end)}})
        .Then([&result](fidl::Result<fuos::Registry::RegisterScopedViewTreeWatcher>& res) {
          ASSERT_TRUE(res.is_ok());
          result = true;
        });
    RunLoopUntil([&result] { return result.has_value(); });
    EXPECT_TRUE(result.value());

    BlockingPresent(this, *parent_session);
  }

  // Set up the child_view and connect it to the parent_view.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  zx_koid_t child_view_ref_koid = ZX_KOID_INVALID;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref_koid = ExtractKoid(identity.view_ref());

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

  // Detach the child_view from the parent_view.
  EXPECT_TRUE((*child_session)->ReleaseView().is_ok());
  BlockingPresent(this, *child_session);

  std::optional<fuog::WatchResponse> view_tree_result;

  view_tree_watcher->Watch().Then(
      [&view_tree_result](fidl::Result<fuog::ViewTreeWatcher::Watch>& response) {
        ASSERT_TRUE(response.is_ok());
        view_tree_result = std::move(response.value());
      });

  RunLoopUntil([&view_tree_result] { return view_tree_result.has_value(); });

  EXPECT_FALSE(view_tree_result->error().has_value());

  ASSERT_TRUE(view_tree_result->updates().has_value());

  // This snapshot captures the state of the view tree when parent_view gets connected to the
  // root_view. The child view had not yet connected at this point, so we only
  // expect to see the parent view.
  {
    auto snapshot_iter =
        GetFirstSnapshotWithView(view_tree_result->updates().value(), parent_view_ref_koid);
    ASSERT_TRUE(snapshot_iter != view_tree_result->updates()->end());
    AssertViewTreeSnapshot(*snapshot_iter, ViewBuilder().AddView(parent_view_ref_koid, {}).Build());
  }

  // This snapshot captures the state of the view tree when child_view gets connected to the
  // parent_view.
  {
    auto snapshot_iter =
        GetFirstSnapshotWithView(view_tree_result->updates().value(), child_view_ref_koid);
    ASSERT_TRUE(snapshot_iter != view_tree_result->updates()->end());
    AssertViewTreeSnapshot(*snapshot_iter, ViewBuilder()
                                               .AddView(parent_view_ref_koid, {child_view_ref_koid})
                                               .AddView(child_view_ref_koid, {})
                                               .Build());
  }

  // This snapshot captures the state of the view tree when child_view detaches from the
  // parent_view.
  {
    // Updates are reversed to find the snapshot having only the parent_view after the
    // child_view gets connected. This represents child_view getting disconnected.
    std::reverse(view_tree_result->updates()->begin(), view_tree_result->updates()->end());
    auto snapshot_iter =
        GetFirstSnapshotWithView(view_tree_result->updates().value(), parent_view_ref_koid);
    ASSERT_TRUE(snapshot_iter != view_tree_result->updates()->end());

    AssertViewTreeSnapshot(*snapshot_iter, ViewBuilder().AddView(parent_view_ref_koid, {}).Build());
  }
}

TEST_F(FlatlandObserverRegistryIntegrationTest, ClientReceivesLayoutUpdatesForFlatland) {
  auto [view_tree_watcher_client_end, view_tree_watcher_server_end] =
      fidl::CreateEndpoints<fuog::ViewTreeWatcher>().value();
  fidl::Client<fuog::ViewTreeWatcher> view_tree_watcher(std::move(view_tree_watcher_client_end),
                                                        dispatcher());

  // Set up the parent_view and connect it to the root_view.
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  zx_koid_t parent_view_ref_koid = ZX_KOID_INVALID;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    parent_view_ref_koid = ExtractKoid(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token));

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    // Register view tree watcher before proceeding.
    // We can't register earlier, because we need to know `parent_view_ref_koid` to register an
    // observer scoped to the parent view.
    std::optional<bool> result;
    scoped_observer_registry_client_
        ->RegisterScopedViewTreeWatcher({{.context_view = parent_view_ref_koid,
                                          .watcher = std::move(view_tree_watcher_server_end)}})
        .Then([&result](fidl::Result<fuos::Registry::RegisterScopedViewTreeWatcher>& res) {
          ASSERT_TRUE(res.is_ok());
          result = true;
        });
    RunLoopUntil([&result] { return result.has_value(); });
    EXPECT_TRUE(result.value());

    BlockingPresent(this, *parent_session);
  }

  // Set up the child_view and connect it to the parent_view.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  zx_koid_t child_view_ref_koid = ZX_KOID_INVALID;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref_koid = ExtractKoid(identity.view_ref());

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

  // Modify the Viewport properties of the child.
  fuc::ViewportProperties properties;
  const int32_t width = 100, height = 100;
  properties.logical_size(fuchsia_math::SizeU{
      {.width = static_cast<uint32_t>(width), .height = static_cast<uint32_t>(height)}});
  EXPECT_TRUE((*parent_session)
                  ->SetViewportProperties(
                      {{.viewport_id = fuc::ContentId(1), .properties = std::move(properties)}})
                  .is_ok());

  BlockingPresent(this, *parent_session);

  std::optional<fuog::WatchResponse> view_tree_result;

  view_tree_watcher->Watch().Then(
      [&view_tree_result](fidl::Result<fuog::ViewTreeWatcher::Watch>& response) {
        ASSERT_TRUE(response.is_ok());
        view_tree_result = std::move(response.value());
      });

  RunLoopUntil([&view_tree_result] { return view_tree_result.has_value(); });

  EXPECT_FALSE(view_tree_result->error().has_value());

  ASSERT_TRUE(view_tree_result->updates().has_value());

  // This snapshot captures the state of the view tree when the parent view sets the logical size
  // of the viewport as {|kDefaultSize|,|kDefaultSize|}.
  {
    // The first snapshot having the child view should represent the state where the layout size of
    // the child view is {|width|,|height|}.
    auto snapshot_iter =
        GetFirstSnapshotWithView(view_tree_result->updates().value(), child_view_ref_koid);
    AssertViewTreeSnapshot(*snapshot_iter, ViewBuilder()
                                               .AddView(parent_view_ref_koid, {child_view_ref_koid},
                                                        std::make_pair(kDefaultSize, kDefaultSize))
                                               .AddView(child_view_ref_koid, {},
                                                        std::make_pair(kDefaultSize, kDefaultSize))
                                               .Build());
  }

  // This snapshot captures the state of the view tree when the parent view sets the logical size
  // of the viewport as {|width|,|height|}.
  {
    // The last snapshot having the child view should represent the state where the layout size of
    // the child view is {|kDefaultSize|,|kDefaultSize|}.
    std::reverse(view_tree_result->updates()->begin(), view_tree_result->updates()->end());
    auto snapshot_iter =
        GetFirstSnapshotWithView(view_tree_result->updates().value(), child_view_ref_koid);
    AssertViewTreeSnapshot(*snapshot_iter,
                           ViewBuilder()
                               .AddView(parent_view_ref_koid, {child_view_ref_koid},
                                        std::make_pair(kDefaultSize, kDefaultSize))
                               .AddView(child_view_ref_koid, {}, std::make_pair(width, height))
                               .Build());
  }
}

// A view present in a fuog_ViewTreeSnapshot must be present in the view tree and should be
// focusable and hittable. In this test, the client (root view) uses |f.u.o.g.Provider| to get
// notified about a child view getting connected and then moves focus to the child view.
TEST_F(FlatlandObserverRegistryIntegrationTest, ChildRequestsFocusAfterConnectingForFlatland) {
  auto [view_tree_watcher_client_end, view_tree_watcher_server_end] =
      fidl::CreateEndpoints<fuog::ViewTreeWatcher>().value();
  fidl::Client<fuog::ViewTreeWatcher> view_tree_watcher(std::move(view_tree_watcher_client_end),
                                                        dispatcher());

  // Set up the child view and connect it to the root view.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  fuv::ViewRef child_view_ref;
  zx_koid_t child_view_ref_koid = ZX_KOID_INVALID;
  auto [child_focused_client_end, child_focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  fidl::Client<fuv::ViewRefFocused> child_focused_ptr(std::move(child_focused_client_end),
                                                      dispatcher());
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    fuc::ViewBoundProtocols protocols;
    protocols.view_ref_focused(std::move(child_focused_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    child_view_ref_koid = ExtractKoid(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token));

    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    // Register view tree watcher before proceeding.
    // We can't register earlier, because we need to know `child_view_ref_koid` to register an
    // observer scoped to the child view.
    std::optional<bool> result;
    scoped_observer_registry_client_
        ->RegisterScopedViewTreeWatcher({{.context_view = child_view_ref_koid,
                                          .watcher = std::move(view_tree_watcher_server_end)}})
        .Then([&result](fidl::Result<fuos::Registry::RegisterScopedViewTreeWatcher>& res) {
          ASSERT_TRUE(res.is_ok());
          result = true;
        });
    RunLoopUntil([&result] { return result.has_value(); });
    EXPECT_TRUE(result.value());

    BlockingPresent(this, *child_session);
  }

  // Watch for child focused event.
  std::optional<bool> child_focused;
  child_focused_ptr->Watch().Then(
      [&child_focused](fidl::Result<fuv::ViewRefFocused::Watch>& update) {
        ASSERT_TRUE(update.is_ok());
        ASSERT_TRUE(update->state().focused().has_value());
        child_focused = update->state().focused().value();
      });

  std::optional<fuog::WatchResponse> view_tree_result;

  view_tree_watcher->Watch().Then(
      [&view_tree_result](fidl::Result<fuog::ViewTreeWatcher::Watch>& response) {
        ASSERT_TRUE(response.is_ok());
        view_tree_result = std::move(response.value());
      });

  RunLoopUntil([&view_tree_result] { return view_tree_result.has_value(); });

  ASSERT_TRUE(view_tree_result->updates().has_value());
  ASSERT_FALSE(view_tree_result->error().has_value());

  // Root view moves focus to the child view after it shows up in the fuog_ViewTreeSnapshot.
  std::optional<bool> request_processed;
  root_focuser_->RequestFocus({{.view_ref = scenic::cpp::CloneViewRef(child_view_ref)}})
      .Then([&request_processed](fidl::Result<fuv::Focuser::RequestFocus>& result) {
        ASSERT_TRUE(result.is_ok());
        request_processed = true;
      });

  RunLoopUntil([&request_processed, &child_focused] {
    return request_processed.has_value() && child_focused.has_value();
  });

  // This snapshot captures the state of the view tree when the child view gets connected to the
  // root view.
  auto snapshot =
      GetFirstSnapshotWithView(view_tree_result->updates().value(), child_view_ref_koid);
  ASSERT_TRUE(snapshot != view_tree_result->updates()->end());

  // Child view should receive focus when it gets connected to the root view.
  EXPECT_TRUE(request_processed.value());
  EXPECT_TRUE(child_focused.value());
}

}  // namespace integration_tests
