// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/geometry_provider.h"

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.observation.geometry/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"
#include "src/ui/scenic/lib/view_tree/tests/utils.h"

namespace view_tree {
namespace geometry_provider::test {

class TestEventHandler
    : public fidl::AsyncEventHandler<fuchsia_ui_observation_geometry::ViewTreeWatcher> {
 public:
  void on_fidl_error(fidl::UnbindInfo info) override { unbind_info = info; }
  std::optional<fidl::UnbindInfo> unbind_info;
};

// Unit tests for testing the fuchsia.ui.observation.geometry.ViewTreeWatcher
// protocol.
// Class fixture for TEST_F.
class GeometryProviderTest : public gtest::TestLoopFixture {
 protected:
  GeometryProviderTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        snapshot_holder_(std::make_shared<SnapshotHolder>()),
        geometry_provider_(snapshot_holder_) {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
    geometry_provider_.Register(std::move(server_end), kNodeA);
    client_.Bind(std::move(client_end), dispatcher(), &event_handler_);

    FX_CHECK(client_.is_valid());
  }

  // Generates |num_snapshots| snapshots with |total_nodes| view nodes and triggers the geometry
  // provider manager to add the newly generated snapshots to all the registered endpoints.
  void PopulateEndpointsWithSnapshots(uint32_t num_snapshots, uint64_t total_nodes) {
    for (uint32_t i = 0; i < num_snapshots; i++) {
      auto snapshot = SingleDepthViewTreeSnapshot(total_nodes, ++sequence_number_);
      snapshot_holder_->SetSnapshot(snapshot);
      geometry_provider_.OnNewViewTreeSnapshot();
    }
  }

  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  std::shared_ptr<SnapshotHolder> snapshot_holder_;
  GeometryProvider geometry_provider_;
  TestEventHandler event_handler_;
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client_;
  uint64_t sequence_number_ = 0;
};

// Clients waiting for a snapshot get a response as soon as a new snapshot is generated.
TEST_F(GeometryProviderTest, SingleWatchBeforeUpdate) {
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  const uint32_t num_snapshots = 1;
  const uint64_t num_nodes = 1;

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });

  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());

  // Clients should not receive any snapshots when no snapshots have been generated.
  EXPECT_FALSE(client_result.has_value());

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
  RunLoopUntilIdle();

  // Clients are sent the new snapshot as soon as a new snapshot is generated.
  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
}

// A Watch call should fail when there is another hanging Watch call by the same client.
TEST_F(GeometryProviderTest, WatchDuringHangingWatch_ShouldFail) {
  client_->Watch().ThenExactlyOnce([](auto& result) {});
  client_->Watch().ThenExactlyOnce([](auto& result) {});

  RunLoopUntilIdle();

  // Client connection is closed since it tried to make another Watch() call when a
  // Watch() call was already in progress.
  EXPECT_TRUE(event_handler_.unbind_info.has_value());
  EXPECT_EQ(event_handler_.unbind_info->status(), ZX_ERR_BAD_STATE);
}

// Clients receive snapshots when there are snapshots queued up from the time the client had
// registered.
TEST_F(GeometryProviderTest, ClientReceivesPendingSnapshots) {
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
  const uint64_t num_nodes = 1;

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });

  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());

  // Client should receive all queued up snapshots.
  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
}

// Client is able to make a successful Watch() call after the previous Watch() call
// finished processing.
TEST_F(GeometryProviderTest, WatchAfterProcessedWatch) {
  {
    std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
    const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
    const uint64_t num_nodes = 1;

    PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

    client_->Watch().ThenExactlyOnce(
        [&client_result](
            fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
          if (result.is_ok()) {
            client_result = std::move(result.value());
          }
        });
    RunLoopUntilIdle();

    EXPECT_TRUE(client_.is_valid());
    ASSERT_TRUE(client_result.has_value());
    ASSERT_TRUE(client_result->updates().has_value());
    EXPECT_EQ(client_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
  }
  {
    std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
    const uint32_t num_snapshots = 1;
    const uint64_t num_nodes = 1;

    client_->Watch().ThenExactlyOnce(
        [&client_result](
            fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
          if (result.is_ok()) {
            client_result = std::move(result.value());
          }
        });
    RunLoopUntilIdle();

    EXPECT_TRUE(client_.is_valid());
    // Client waits for new snapshots to consume since there are no new snapshots generated after
    // the previous Watch() call.
    ASSERT_FALSE(client_result.has_value());

    PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
    RunLoopUntilIdle();

    // Client receives the latest generated snapshot.
    ASSERT_TRUE(client_result.has_value());
    ASSERT_TRUE(client_result->updates().has_value());
    EXPECT_EQ(client_result->updates()->size(), 1UL);
  }
}

// In case the number of snapshots queued up before the next Watch() call is greater than
// BUFFER_SIZE, only the latest BUFFER_SIZE snapshots are returned and the old snapshots are
// discarded.
TEST_F(GeometryProviderTest, BufferOverflowTest) {
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
  const uint64_t num_nodes = 1;

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes + 1);

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });

  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());
  ASSERT_TRUE(client_result.has_value());

  // Client should receive the latest BUFFER_SIZE snapshot updates. The latest snapshots have
  // |num_nodes|+1 view nodes.
  ASSERT_TRUE(client_result->error().has_value());
  EXPECT_TRUE(static_cast<uint32_t>(*client_result->error() &
                                    fuchsia_ui_observation_geometry::Error::kBufferOverflow));
  ASSERT_TRUE(client_result->updates().has_value());
  for (auto& snapshot : *client_result->updates()) {
    ASSERT_TRUE(snapshot.views().has_value());
    EXPECT_EQ(snapshot.views()->size(), num_nodes + 1);
  }
}

// Clients registered with the protocol should be receiving updates even if one of the clients is
// killed for making an illegal Watch() call.
TEST_F(GeometryProviderTest, MisbehavingClientsShouldNotAffectOtherClients) {
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client1;
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client2;
  auto [c1_client, c1_server] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  auto [c2_client, c2_server] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.Register(std::move(c1_server), kNodeA);
  geometry_provider_.Register(std::move(c2_server), kNodeA);
  TestEventHandler client1_event_handler;
  client1.Bind(std::move(c1_client), dispatcher(), &client1_event_handler);
  client2.Bind(std::move(c2_client), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client1_result;
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client2_result;
  const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
  const uint64_t num_nodes = 1;

  // Client makes an illegal Watch() call resulting in it being killed.
  client1->Watch().ThenExactlyOnce(
      [&client1_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client1_result = std::move(result.value());
        }
      });
  client1->Watch().ThenExactlyOnce(
      [&client1_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client1_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client1_event_handler.unbind_info.has_value());
  EXPECT_EQ(client1_event_handler.unbind_info->status(), ZX_ERR_BAD_STATE);

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  client2->Watch().ThenExactlyOnce(
      [&client2_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client2_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());
  EXPECT_TRUE(client2.is_valid());

  // Other clients should still receive pending snapshot updates despite client1 getting killed.
  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client2_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  ASSERT_TRUE(client2_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
  EXPECT_EQ(client2_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
}

// Other clients should still receive pending snapshot updates even if any other client dies.
TEST_F(GeometryProviderTest, ClientFailuresShouldNotAffectOtherClients) {
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client1;
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client2;
  auto [c1_client, c1_server] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  auto [c2_client, c2_server] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.Register(std::move(c1_server), kNodeA);
  geometry_provider_.Register(std::move(c2_server), kNodeA);
  client1.Bind(std::move(c1_client), dispatcher());
  client2.Bind(std::move(c2_client), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client1_result;
  const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
  const uint64_t num_nodes = 1;

  // client2 closes the channel to mock client death.
  client2 = {};

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  client1->Watch().ThenExactlyOnce(
      [&client1_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client1_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());
  EXPECT_TRUE(client1.is_valid());
  EXPECT_FALSE(client2.is_valid());

  // Other clients should still receive pending snapshot updates despite client2 dying.
  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client1_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  ASSERT_TRUE(client1_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
  EXPECT_EQ(client1_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
}

TEST_F(GeometryProviderTest, ClientDoesNotReceiveViews_WhenViewsCountExceedMaxViewAllowed) {
  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  const uint32_t num_snapshots = 1;
  const uint64_t num_nodes = fuchsia_ui_observation_geometry::kMaxViewCount * 2;

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  client_->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());

  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  ASSERT_EQ(client_result->updates()->size(), 1UL);

  // The client will not receive a views vector in the response as the size of the views vector
  // would have exceeded kMaxViewCount.
  EXPECT_FALSE((*client_result->updates())[0].views().has_value());
}

// A Watch() call should succeed when size of the response exceeds the maximum size of a
// message that can be sent over the FIDL channel.
TEST_F(GeometryProviderTest, WatchShouldSucceed_WhenResponseSizeExceedsFIDLChannelMaxSize) {
  // The total number of ViewTreeSnapshots will always be less than BUFFER_SIZE when
  // the response size exceeds FIDL channel's limit.
  {
    std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
    const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
    const uint64_t num_nodes = 10;

    PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

    client_->Watch().ThenExactlyOnce(
        [&client_result](
            fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
          if (result.is_ok()) {
            client_result = std::move(result.value());
          }
        });
    RunLoopUntilIdle();

    EXPECT_TRUE(client_.is_valid());

    ASSERT_TRUE(client_result.has_value());

    ASSERT_TRUE(client_result->error().has_value());
    EXPECT_TRUE(static_cast<uint32_t>(*client_result->error() &
                                      fuchsia_ui_observation_geometry::Error::kChannelOverflow));
    ASSERT_TRUE(client_result->updates().has_value());
    EXPECT_LT(client_result->updates()->size(), fuchsia_ui_observation_geometry::kBufferSize);
  }
  // The response should contain ViewTreeSnapshot generated from the most recent snapshot
  // when the response size exceeds the FIDL channel's limit.
  {
    std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;

    {
      const uint32_t num_snapshots = 1;
      const uint64_t num_nodes = fuchsia_ui_observation_geometry::kMaxViewCount;
      PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
    }
    {
      const uint32_t num_snapshots = 1;
      const uint64_t num_nodes = fuchsia_ui_observation_geometry::kMaxViewCount - 10;
      PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
    }
    {
      const uint32_t num_snapshots = 1;
      const uint64_t num_nodes = fuchsia_ui_observation_geometry::kMaxViewCount - 100;
      PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);
    }

    client_->Watch().ThenExactlyOnce(
        [&client_result](
            fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
          if (result.is_ok()) {
            client_result = std::move(result.value());
          }
        });
    RunLoopUntilIdle();

    EXPECT_TRUE(client_.is_valid());

    // As the number of view nodes in the view tree of the 3 snapshots are large, including
    // ViewTreeSnapshots generated from more than 1 snapshot in the response will exceed
    // FIDL channel's limit. Therefore, the server only sends the ViewTreeSnapshot generated
    // from the latest snapshot to the client.
    ASSERT_TRUE(client_result.has_value());

    ASSERT_TRUE(client_result->error().has_value());
    EXPECT_TRUE(static_cast<uint32_t>(*client_result->error() &
                                      fuchsia_ui_observation_geometry::Error::kChannelOverflow));
    ASSERT_TRUE(client_result->updates().has_value());
    EXPECT_EQ(client_result->updates()->size(), 1UL);
    ASSERT_TRUE((*client_result->updates())[0].views().has_value());
    EXPECT_EQ((*client_result->updates())[0].views()->size(),
              fuchsia_ui_observation_geometry::kMaxViewCount - 100);
  }
}

// ViewDescriptor should accurately capture data from a view_tree::ViewNode. The test uses
// the following three node topology:
// node_a (root)
//  |
// node_b
//  |
// node_c
TEST_F(GeometryProviderTest, ExtractObservationSnapshotTest) {
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  zx_koid_t node_a_koid = 1, node_b_koid = 2, node_c_koid = 3;
  auto node_a = ViewNode{.children = {node_b_koid}};
  auto node_b = ViewNode{.parent = node_a_koid, .children = {node_c_koid}};
  auto node_c = ViewNode{.parent = node_b_koid};

  // Set up node_a.
  {
    const uint32_t width = 10, height = 10;
    view_tree::BoundingBox bounding_box = {.min = {0, 0}, .max = {width, height}};
    node_a.bounding_box = std::move(bounding_box);
  }

  // Set up node_b.
  {
    const uint32_t width = 5, height = 5;
    view_tree::BoundingBox bounding_box = {.min = {0, 0}, .max = {width, height}};
    node_b.bounding_box = std::move(bounding_box);
  }

  // Set up node_c.
  {
    const uint32_t width = 1, height = 1;
    view_tree::BoundingBox bounding_box = {.min = {0, 0}, .max = {width, height}};
    node_c.bounding_box = std::move(bounding_box);
  }

  // Client should receive an empty views vector in the response when a view_tree::Snapshot has no
  // views.
  {
    auto view_tree_snapshot = view_tree::GeometryProvider::ExtractObservationSnapshot(
        /*context_view*/ std::nullopt, *snapshot);
    ASSERT_TRUE(view_tree_snapshot.has_value());
    ASSERT_TRUE(view_tree_snapshot->views().has_value());
    EXPECT_TRUE(view_tree_snapshot->views()->empty());
  }

  snapshot->root = node_a_koid;
  snapshot->view_tree.try_emplace(node_a_koid, std::move(node_a));
  snapshot->view_tree.try_emplace(node_b_koid, std::move(node_b));
  snapshot->view_tree.try_emplace(node_c_koid, std::move(node_c));

  // Client should receive ViewDescriptor for every node in the view tree since the root node
  // is the context view.
  {
    auto view_tree_snapshot = view_tree::GeometryProvider::ExtractObservationSnapshot(
        /*context_view*/ node_a_koid, *snapshot);

    ASSERT_TRUE(view_tree_snapshot.has_value());
    ASSERT_TRUE(view_tree_snapshot->views().has_value());
    ASSERT_EQ(view_tree_snapshot->views()->size(), 3UL);

    // ViewDescriptor for node_a.
    {
      auto& vd = (*view_tree_snapshot->views())[0];

      ASSERT_TRUE(vd.view_ref_koid().has_value());
      EXPECT_EQ(*vd.view_ref_koid(), node_a_koid);

      ASSERT_TRUE(vd.layout().has_value());
      auto& layout = *vd.layout();
      auto node_logical_width =
          static_cast<float>(snapshot->view_tree[node_a_koid].bounding_box.max[0]);
      auto node_logical_height =
          static_cast<float>(snapshot->view_tree[node_a_koid].bounding_box.max[1]);

      // Minimum coordinates for a layout should be its origin and maximum coordinates should be
      // equal to the node's logical size.
      EXPECT_FLOAT_EQ(layout.extent().min().x(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().min().y(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().max().x(), node_logical_width);
      EXPECT_FLOAT_EQ(layout.extent().max().y(), node_logical_height);

      ASSERT_TRUE(vd.extent_in_context().has_value());
      auto& extent_in_context = *vd.extent_in_context();

      // For the context view, |extent_in_context| should be the same as its |layout|.
      EXPECT_FLOAT_EQ(extent_in_context.origin().x(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.origin().y(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.width(), node_logical_width);
      EXPECT_FLOAT_EQ(extent_in_context.height(), node_logical_height);
      EXPECT_FLOAT_EQ(extent_in_context.angle_degrees(), 0.);

      // The context view isn't allowed to see into the parent.
      ASSERT_FALSE(vd.extent_in_parent().has_value());

      ASSERT_TRUE(vd.children().has_value());
      EXPECT_THAT(*vd.children(),
                  testing::UnorderedElementsAre(static_cast<uint32_t>(node_b_koid)));
    }

    // ViewDescriptor for node_b.
    {
      auto& vd = (*view_tree_snapshot->views())[1];

      ASSERT_TRUE(vd.view_ref_koid().has_value());
      EXPECT_EQ(*vd.view_ref_koid(), node_b_koid);

      ASSERT_TRUE(vd.layout().has_value());
      auto& layout = *vd.layout();
      auto node_logical_width =
          static_cast<float>(snapshot->view_tree[node_b_koid].bounding_box.max[0]);
      auto node_logical_height =
          static_cast<float>(snapshot->view_tree[node_b_koid].bounding_box.max[1]);

      EXPECT_FLOAT_EQ(layout.extent().min().x(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().min().y(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().max().x(), node_logical_width);
      EXPECT_FLOAT_EQ(layout.extent().max().y(), node_logical_height);

      ASSERT_TRUE(vd.extent_in_context().has_value());
      auto& extent_in_context = *vd.extent_in_context();

      // As all the nodes in the view_tree have |local_from_world_transform| as identity matrix,
      // |extent_in_context| and |extent_in_parent| will be the same as layout.
      EXPECT_FLOAT_EQ(extent_in_context.origin().x(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.origin().y(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.width(), node_logical_width);
      EXPECT_FLOAT_EQ(extent_in_context.height(), node_logical_height);
      EXPECT_FLOAT_EQ(extent_in_context.angle_degrees(), 0.);

      ASSERT_TRUE(vd.extent_in_parent().has_value());
      auto& extent_in_parent = *vd.extent_in_parent();

      EXPECT_FLOAT_EQ(extent_in_parent.origin().x(), 0.);
      EXPECT_FLOAT_EQ(extent_in_parent.origin().y(), 0.);
      EXPECT_FLOAT_EQ(extent_in_parent.width(), node_logical_width);
      EXPECT_FLOAT_EQ(extent_in_parent.height(), node_logical_height);
      EXPECT_FLOAT_EQ(extent_in_parent.angle_degrees(), 0.);

      ASSERT_TRUE(vd.children().has_value());
      EXPECT_THAT(*vd.children(),
                  testing::UnorderedElementsAre(static_cast<uint32_t>(node_c_koid)));
    }

    // ViewDescriptor for node_c.
    {
      auto& vd = (*view_tree_snapshot->views())[2];

      ASSERT_TRUE(vd.view_ref_koid().has_value());
      EXPECT_EQ(*vd.view_ref_koid(), node_c_koid);

      ASSERT_TRUE(vd.layout().has_value());
      auto& layout = *vd.layout();
      auto node_logical_width =
          static_cast<float>(snapshot->view_tree[node_c_koid].bounding_box.max[0]);
      auto node_logical_height =
          static_cast<float>(snapshot->view_tree[node_c_koid].bounding_box.max[1]);

      EXPECT_FLOAT_EQ(layout.extent().min().x(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().min().y(), 0.);
      EXPECT_FLOAT_EQ(layout.extent().max().x(), node_logical_width);
      EXPECT_FLOAT_EQ(layout.extent().max().y(), node_logical_height);

      ASSERT_TRUE(vd.extent_in_context().has_value());
      auto& extent_in_context = *vd.extent_in_context();

      EXPECT_FLOAT_EQ(extent_in_context.origin().x(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.origin().y(), 0.);
      EXPECT_FLOAT_EQ(extent_in_context.width(), node_logical_width);
      EXPECT_FLOAT_EQ(extent_in_context.height(), node_logical_height);
      EXPECT_FLOAT_EQ(extent_in_context.angle_degrees(), 0.);

      ASSERT_TRUE(vd.extent_in_parent().has_value());
      auto& extent_in_parent = *vd.extent_in_parent();

      EXPECT_FLOAT_EQ(extent_in_parent.origin().x(), 0.);
      EXPECT_FLOAT_EQ(extent_in_parent.origin().y(), 0.);
      EXPECT_FLOAT_EQ(extent_in_parent.width(), node_logical_width);
      EXPECT_FLOAT_EQ(extent_in_parent.height(), node_logical_height);
      EXPECT_FLOAT_EQ(extent_in_parent.angle_degrees(), 0.);

      ASSERT_TRUE(vd.children().has_value());
      EXPECT_TRUE(vd.children()->empty());
    }
  }

  // Client should receive ViewDescriptor for the context_view only as the context_view is a
  // leaf node.
  {
    auto view_tree_snapshot = view_tree::GeometryProvider::ExtractObservationSnapshot(
        /*context_view*/ node_c_koid, *snapshot);

    ASSERT_TRUE(view_tree_snapshot.has_value());
    ASSERT_TRUE(view_tree_snapshot->views().has_value());
    ASSERT_EQ(view_tree_snapshot->views()->size(), 1UL);

    auto& vd = (*view_tree_snapshot->views())[0];
    ASSERT_TRUE(vd.view_ref_koid().has_value());
    EXPECT_EQ(*vd.view_ref_koid(), node_c_koid);
  }
}

// Clients registered through |RegisterGlobalViewTreeWatcher| should receive information about all
// the nodes in a view tree.
TEST_F(GeometryProviderTest, RegisterGlobalViewTreeWatcherTest) {
  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.RegisterGlobalViewTreeWatcher(std::move(server_end));
  client.Bind(std::move(client_end), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  const uint32_t num_snapshots = 1;
  const uint64_t num_nodes = 5;

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });

  RunLoopUntilIdle();

  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_FALSE(client_result->error().has_value());
  ASSERT_EQ(client_result->updates()->size(), num_snapshots);

  // Client should receive ViewDescriptors for |num_nodes| since it has a unlimited access to
  // the global view tree.
  ASSERT_TRUE((*client_result->updates())[0].views().has_value());
  EXPECT_EQ((*client_result->updates())[0].views()->size(), num_nodes);
}

// Clients registered using |fuchsia.ui.observation.scope.Registry| get updates about its
// |context_view| and other descendant views.
TEST_F(GeometryProviderTest, ScopedRegistryTest) {
  const zx_koid_t node_a_koid = 1, node_b_koid = 2;
  const float width = 1, height = 1;

  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.Register(std::move(server_end), node_b_koid);
  client.Bind(std::move(client_end), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;

  // Generate an empty view tree snapshot.
  {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    snapshot->sequence_number = 1;
    snapshot_holder_->SetSnapshot(snapshot);
    geometry_provider_.OnNewViewTreeSnapshot();
  }

  client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client.is_valid());

  ASSERT_TRUE(client_result.has_value());

  // Client receives an empty views vector in the response when the view tree is empty.
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
  ASSERT_TRUE((*client_result->updates())[0].views().has_value());
  EXPECT_TRUE((*client_result->updates())[0].views()->empty());

  // Generate a snapshot containing only |node_a|.
  {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    auto node_a = ViewNode{.bounding_box = {.min = {0, 0}, .max = {width, height}}};
    snapshot->root = node_a_koid;
    snapshot->view_tree.try_emplace(node_a_koid, std::move(node_a));
    snapshot->sequence_number = 2;
    snapshot_holder_->SetSnapshot(snapshot);
    geometry_provider_.OnNewViewTreeSnapshot();
  }

  client_result.reset();
  client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client.is_valid());

  ASSERT_TRUE(client_result.has_value());

  // Client receives an empty views vector in the response as its |context_view| is not present in
  // the view tree.
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
  ASSERT_TRUE((*client_result->updates())[0].views().has_value());
  EXPECT_TRUE((*client_result->updates())[0].views()->empty());

  // Generate a snapshot with |node_a| as the root and |node_b| as the child of |node_a|.
  {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    auto node_a = ViewNode{.children = {node_b_koid},
                           .bounding_box = {.min = {0, 0}, .max = {width, height}}};
    auto node_b =
        ViewNode{.parent = node_a_koid, .bounding_box = {.min = {0, 0}, .max = {width, height}}};
    snapshot->root = node_a_koid;
    snapshot->view_tree.try_emplace(node_a_koid, std::move(node_a));
    snapshot->view_tree.try_emplace(node_b_koid, std::move(node_b));
    snapshot->sequence_number = 3;
    snapshot_holder_->SetSnapshot(snapshot);
    geometry_provider_.OnNewViewTreeSnapshot();
  }

  client_result.reset();
  client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client.is_valid());

  ASSERT_TRUE(client_result.has_value());

  // Client receives updates about its |context_view| in the response as it is now present in the
  // view tree.
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
  ASSERT_TRUE((*client_result->updates())[0].views().has_value());
  EXPECT_EQ((*client_result->updates())[0].views()->size(), 1UL);
}

TEST_F(GeometryProviderTest, ZeroSizedWindows_AreOmitted) {
  const zx_koid_t node_a_koid = 1, node_b_koid = 2;

  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> client;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.Register(std::move(server_end), node_b_koid);
  client.Bind(std::move(client_end), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;

  // Generate a snapshot with |node_a| as the root and |node_b| as the child of |node_a|.
  // This time, however, the views are zero-sized.
  {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    auto node_a =
        ViewNode{.children = {node_b_koid}, .bounding_box = {.min = {0, 0}, .max = {0, 0}}};
    auto node_b = ViewNode{.parent = node_a_koid, .bounding_box = {.min = {0, 0}, .max = {0, 0}}};
    snapshot->root = node_a_koid;
    snapshot->view_tree.try_emplace(node_a_koid, std::move(node_a));
    snapshot->view_tree.try_emplace(node_b_koid, std::move(node_b));
    snapshot->sequence_number = 1;
    snapshot_holder_->SetSnapshot(snapshot);
    geometry_provider_.OnNewViewTreeSnapshot();
  }

  client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });
  RunLoopUntilIdle();

  EXPECT_TRUE(client.is_valid());

  ASSERT_TRUE(client_result.has_value());

  // Client receives updates about its |context_view|.
  // However, the zero-sized views are not listed.
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
  ASSERT_TRUE((*client_result->updates())[0].views().has_value());
  EXPECT_EQ((*client_result->updates())[0].views()->size(), 0UL);
}

// If there was a previous update before a client is registered, then the first watch should succeed
// even if there isn't a subsequent update.
TEST_F(GeometryProviderTest, UpdateBeforeWatch) {
  const uint32_t num_snapshots = fuchsia_ui_observation_geometry::kBufferSize;
  const uint64_t num_nodes = 1;

  PopulateEndpointsWithSnapshots(num_snapshots, num_nodes);

  fidl::Client<fuchsia_ui_observation_geometry::ViewTreeWatcher> new_client;
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_ui_observation_geometry::ViewTreeWatcher>::Create();
  geometry_provider_.Register(std::move(server_end), kNodeA);
  new_client.Bind(std::move(client_end), dispatcher());

  std::optional<fuchsia_ui_observation_geometry::WatchResponse> client_result;
  new_client->Watch().ThenExactlyOnce(
      [&client_result](
          fidl::Result<fuchsia_ui_observation_geometry::ViewTreeWatcher::Watch>& result) {
        if (result.is_ok()) {
          client_result = std::move(result.value());
        }
      });

  RunLoopUntilIdle();

  EXPECT_TRUE(client_.is_valid());
  ASSERT_TRUE(client_result.has_value());
  ASSERT_TRUE(client_result->updates().has_value());
  EXPECT_EQ(client_result->updates()->size(), 1UL);
}

}  // namespace geometry_provider::test
}  // namespace view_tree
