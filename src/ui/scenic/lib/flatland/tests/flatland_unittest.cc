// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/default.h>
#include <lib/async/time.h>
#include <lib/fpromise/bridge.h>
#include <lib/sync/cpp/completion.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <unordered_set>

#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/lib/escher/util/epsilon_compare.h"
#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/flatland_display.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/scene_dumper.h"
#include "src/ui/scenic/lib/flatland/tests/flatland_unittest.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/scenic/util/error_reporter.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/utils/dispatcher_holder.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"
#include "zircon/errors.h"

#include <glm/gtx/matrix_transform_2d.hpp>

using ::testing::_;
using ::testing::Return;

using BufferCollectionId = flatland::Flatland::BufferCollectionId;
using allocation::Allocator;
using allocation::BufferCollectionImporter;
using allocation::ImageMetadata;
using allocation::MockBufferCollectionImporter;
using allocation::cpp::BufferCollectionImportExportTokens;
using flatland::ContentId;
using flatland::EventHandler;
using flatland::Flatland;
using flatland::Flatland2Test;
using flatland::FlatlandConfig;
using flatland::FlatlandDisplay;
using flatland::FlatlandPresenter;
using flatland::FlatlandTest;
using flatland::GlobalIdPair;
using flatland::GlobalMatrixVector;
using flatland::GlobalTopologyData;
using flatland::kDefaultDisplayPixelRatio;
using flatland::kDefaultInset;
using flatland::kDefaultSize;
using flatland::kInvalidContentId;
using flatland::kInvalidLayerId;
using flatland::kInvalidTransformId;
using flatland::LayerId;
using flatland::LayerStackId;
using flatland::LinkSystem;
using flatland::MockFlatlandPresenter;
using flatland::NoViewProtocols;
using flatland::PresentArgs;
using flatland::SrcToDest;
using flatland::TestErrorReporter;
using flatland::TransformGraph;
using flatland::TransformHandle;
using flatland::TransformId;
using flatland::UberStruct;
using flatland::UberStructSystem;
using fuchsia_math::SizeU;
using fuchsia_ui_composition::BufferCollectionExportToken;
using fuchsia_ui_composition::BufferCollectionImportToken;
using fuchsia_ui_composition::ChildViewStatus;
using fuchsia_ui_composition::ChildViewWatcher;
using fuchsia_ui_composition::FlatlandError;
using fuchsia_ui_composition::ImageProperties;
using fuchsia_ui_composition::LayoutInfo;
using fuchsia_ui_composition::OnNextFrameBeginValues;
using fuchsia_ui_composition::Orientation;
using fuchsia_ui_composition::ParentViewportStatus;
using fuchsia_ui_composition::ParentViewportWatcher;
using fuchsia_ui_composition::ViewportProperties;
using fuchsia_ui_views::ViewCreationToken;
using fuchsia_ui_views::ViewportCreationToken;
using integration_tests::ReturnPromise;
using ChildViewWatcher_GetStatusResult =
    fidl::Result<fuchsia_ui_composition::ChildViewWatcher::GetStatus>;
using ChildViewWatcher_GetViewRefResult =
    fidl::Result<fuchsia_ui_composition::ChildViewWatcher::GetViewRef>;
using ParentViewportWatcher_GetLayoutResult =
    fidl::Result<fuchsia_ui_composition::ParentViewportWatcher::GetLayout>;
using ParentViewportWatcher_GetStatusResult =
    fidl::Result<fuchsia_ui_composition::ParentViewportWatcher::GetStatus>;

namespace {

// Adds FlatlandDisplay-specific helpers to FlatlandTest. We can't easily put them into standalone
// helper functions because they use `Present()`, which expects to have access to protected members
// of FlatlandTest.
class FlatlandDisplayTest : public FlatlandTest {
 protected:
  void ConnectChildViewToDisplayThenValidate(const std::shared_ptr<FlatlandDisplay>& display,
                                             const std::shared_ptr<Flatland>& child,
                                             const uint32_t kWidth, const uint32_t kHeight) {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::Endpoints<ParentViewportWatcher>::Create();
    fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end),
                                         dispatcher());

    SetDisplayContent(display.get(), child.get(), std::move(child_view_watcher_server_end),
                      std::move(parent_viewport_watcher_server_end));

    std::optional<LayoutInfo> layout_info;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout_info = result->info();
        });

    std::optional<ParentViewportStatus> parent_viewport_watcher_status;
    parent_viewport_watcher->GetStatus().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetStatusResult& result) {
          ASSERT_TRUE(result.is_ok());
          parent_viewport_watcher_status = result->status();
        });

    RunLoopUntilIdle();

    // LayoutInfo is sent as soon as the content/graph link is established, to allow clients to
    // generate their first frame with minimal latency.
    ASSERT_TRUE(layout_info.has_value());
    EXPECT_EQ(layout_info->logical_size()->width(), kWidth);
    EXPECT_EQ(layout_info->logical_size()->height(), kHeight);

    // The ParentViewportWatcher's status must wait until the first frame is generated (represented
    // here by the call to UpdateLinks() below).
    EXPECT_FALSE(parent_viewport_watcher_status.has_value());

    UpdateLinks(display->root_transform());

    // UpdateLinks() causes us to receive the status notification.  The link is considered to be
    // disconnected because the child has not yet presented its first frame.
    EXPECT_TRUE(parent_viewport_watcher_status.has_value());
    EXPECT_EQ(parent_viewport_watcher_status.value(),
              ParentViewportStatus::kDisconnectedFromDisplay);
    parent_viewport_watcher_status.reset();

    Present(child, true);

    // The status won't change to "connected" until UpdateLinks() is called again.
    parent_viewport_watcher->GetStatus().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetStatusResult& result) {
          ASSERT_TRUE(result.is_ok());
          parent_viewport_watcher_status = result->status();
        });
    RunLoopUntilIdle();
    EXPECT_FALSE(parent_viewport_watcher_status.has_value());

    UpdateLinks(display->root_transform());

    EXPECT_TRUE(parent_viewport_watcher_status.has_value());
    EXPECT_EQ(parent_viewport_watcher_status.value(), ParentViewportStatus::kConnectedToDisplay);
  }
};

template <typename T>
bool ClientEndPeerExists(const fidl::ClientEnd<T>& client_end) {
  auto result =
      client_end.channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), nullptr);
  if (result == ZX_ERR_TIMED_OUT) {
    return true;
  }
  FX_DCHECK(result == ZX_OK) << "unexpected result from channel().wait_one(): " << result;
  return false;
}

bool PeerExists(const zx::unowned_channel& channel) {
  auto result = channel->wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), nullptr);
  if (result == ZX_ERR_TIMED_OUT) {
    return true;
  }
  FX_DCHECK(result == ZX_OK) << "unexpected result from channel().wait_one(): " << result;
  return false;
}

}  // namespace

namespace flatland::test {

TEST_F(FlatlandTest, PresentShouldReturnSuccess) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  Present(flatland, true);
}

TEST_F(FlatlandTest, PresentErrorNoTokens) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Present, but return no tokens so the client has none left.
  {
    PresentArgs args;
    args.present_credits_returned = 0;
    PresentWithArgs(flatland, std::move(args), true);
  }

  // Present again, which should fail because the client has no tokens.
  {
    PresentArgs args;
    args.expected_error = FlatlandError::kNoPresentsRemaining;
    PresentWithArgs(flatland, std::move(args), false);
  }
}

TEST_F(FlatlandTest, MultiplePresentTokensAvailable) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Return one extra present token, meaning the instance now has two.
  flatland->OnNextFrameBegin(1, {});

  // Present, but return no tokens so the client has only one left.
  {
    PresentArgs args;
    args.present_credits_returned = 0;
    PresentWithArgs(flatland, std::move(args), true);
  }

  // Present again, which should succeed because the client already has an extra token even though
  // the previous PresentWithArgs() returned none.
  {
    PresentArgs args;
    args.present_credits_returned = 0;
    PresentWithArgs(flatland, std::move(args), true);
  }

  // A third Present() will fail since the previous two calls consumed the two tokens.
  {
    PresentArgs args;
    args.expected_error = FlatlandError::kNoPresentsRemaining;
    PresentWithArgs(flatland, std::move(args), false);
  }
}

TEST_F(FlatlandTest, PresentWithNoFieldsSet) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const bool kDefaultUnsquashable = false;
  const zx::time kDefaultRequestedPresentationTime = zx::time(0);

  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(kDefaultRequestedPresentationTime, _, kDefaultUnsquashable,
                                       _, _, _, _));
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, PresentWaitsForAcquireFences) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create two events to serve as acquire fences.
  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(2);
  auto acquire1_copy = utils::CopyZxHandle(args.acquire_fences[0]);
  auto acquire2_copy = utils::CopyZxHandle(args.acquire_fences[1]);

  // Create an event to serve as a release fence.
  args.release_fences = utils::CreateEventArray(1);
  auto release_copy = utils::CopyZxHandle(args.release_fences[0]);

  // Because the Present includes unsignaled acquire fences, the UberStructSystem shouldn't have any
  // entries and applying session updates shouldn't signal the release fence.
  PresentWithArgs(flatland, std::move(args), true);

  auto registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);
  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);
  EXPECT_FALSE(utils::IsEventSignalled(release_copy, ZX_EVENT_SIGNALED));

  // Signal the second fence and verify the UberStructSystem doesn't update, and the release fence
  // isn't signaled.
  acquire2_copy.signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  ApplySessionUpdatesAndSignalFences();

  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);
  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);
  EXPECT_FALSE(utils::IsEventSignalled(release_copy, ZX_EVENT_SIGNALED));

  // Signal the first fence and verify that, the UberStructSystem contains an UberStruct for the
  // instance, and the release fence is signaled.
  acquire1_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();

  // After signaling the final acquire fence (and running the loop), there is now a registered
  // present.  There is still no UberStruct, nor has the release fence been signalled, because
  // session updates haven't been applied.
  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 1ul);
  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);
  EXPECT_FALSE(utils::IsEventSignalled(release_copy, ZX_EVENT_SIGNALED));

  ApplySessionUpdatesAndSignalFences();

  // There is no longer a registered present; it was removed because it was processed when
  // session updates were applied.  There is now an UberStruct, and the release fence has been
  // signaled.
  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);
  EXPECT_NE(GetUberStruct(flatland.get()), nullptr);
  EXPECT_TRUE(utils::IsEventSignalled(release_copy, ZX_EVENT_SIGNALED));
}

TEST_F(FlatlandTest, PresentForwardsRequestedPresentationTime) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create an event to serve as an acquire fence.
  const zx::time requested_presentation_time = zx::time(123);

  PresentArgs args;
  args.requested_presentation_time = requested_presentation_time;
  args.acquire_fences = utils::CreateEventArray(1);
  auto acquire_copy = utils::CopyZxHandle(args.acquire_fences[0]);

  // Because the Present includes acquire fences, it isn't registered with the presenter immediately
  // upon `Present()`.
  auto present_id = scheduling::PeekNextPresentId();
  PresentWithArgs(flatland, std::move(args), true);

  auto registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);

  const auto id_pair = scheduling::SchedulingIdPair({
      .session_id = flatland->GetRoot().GetInstanceId(),
      .present_id = present_id,
  });

  auto maybe_presentation_time = GetRequestedPresentationTime(id_pair);
  EXPECT_FALSE(maybe_presentation_time.has_value());

  // Signal the fence and ensure the Present is still registered, but now with a requested
  // presentation time.
  acquire_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();

  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 1ul);

  maybe_presentation_time = GetRequestedPresentationTime(id_pair);
  EXPECT_TRUE(maybe_presentation_time.has_value());
  EXPECT_EQ(maybe_presentation_time.value(), requested_presentation_time);
}

TEST_F(FlatlandTest, PresentWithSignaledFencesUpdatesImmediately) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create an event to serve as the acquire fence.
  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(1);
  auto acquire_copy = utils::CopyZxHandle(args.acquire_fences[0]);

  // Create an event to serve as a release fence.
  args.release_fences = utils::CreateEventArray(1);
  auto release_copy = utils::CopyZxHandle(args.release_fences[0]);

  // Signal the event before the Present() call.
  acquire_copy.signal(0, ZX_EVENT_SIGNALED);

  // The PresentId is no longer registered because it has been applied, the UberStructSystem should
  // update immediately, and the release fence should be signaled. The PresentWithArgs() method
  // only expects the ScheduleUpdateForSession() call when no acquire fences are present, but since
  // this test specifically tests pre-signaled fences, the EXPECT_CALL must be added here.
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  PresentWithArgs(flatland, std::move(args), true);

  auto registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_TRUE(registered_presents.empty());

  EXPECT_NE(GetUberStruct(flatland.get()), nullptr);

  EXPECT_TRUE(utils::IsEventSignalled(release_copy, ZX_EVENT_SIGNALED));
}

TEST_F(FlatlandTest, PresentsUpdateInCallOrder) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create an event to serve as the acquire fence for the first Present().
  PresentArgs args1;
  args1.acquire_fences = utils::CreateEventArray(1);
  auto acquire1_copy = utils::CopyZxHandle(args1.acquire_fences[0]);

  // Create an event to serve as a release fence.
  args1.release_fences = utils::CreateEventArray(1);
  auto release1_copy = utils::CopyZxHandle(args1.release_fences[0]);

  // Present, but do not signal the fence, and ensure Present is registered, the UberStructSystem is
  // empty, and the release fence is unsignaled.
  PresentWithArgs(flatland, std::move(args1), true);

  // No presents have been registered since the acquire fence hasn't been signaled yet.
  auto registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);
  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);
  EXPECT_FALSE(utils::IsEventSignalled(release1_copy, ZX_EVENT_SIGNALED));

  // Create a transform and make it the root.
  const TransformId kId(1);

  flatland->CreateTransform(kId);
  flatland->SetRootTransform(kId);

  // Create another event to serve as the acquire fence for the second Present().
  PresentArgs args2;
  args2.acquire_fences = utils::CreateEventArray(1);
  auto acquire2_copy = utils::CopyZxHandle(args2.acquire_fences[0]);

  // Create an event to serve as a release fence.
  args2.release_fences = utils::CreateEventArray(1);
  auto release2_copy = utils::CopyZxHandle(args2.release_fences[0]);

  // Present, but do not signal the fence, and ensure there are two Presents registered, but the
  // UberStructSystem is still empty and both release fences are unsignaled.
  PresentWithArgs(flatland, std::move(args2), true);

  // No presents have been registered since neither of the acquire fences have been signaled yet.
  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);
  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);
  EXPECT_FALSE(utils::IsEventSignalled(release1_copy, ZX_EVENT_SIGNALED));
  EXPECT_FALSE(utils::IsEventSignalled(release2_copy, ZX_EVENT_SIGNALED));

  // Signal the fence for the second Present(). Since the first one is not done, there should still
  // be no Presents registered, no UberStruct for the instance, and neither fence should be
  // signaled.
  acquire2_copy.signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  ApplySessionUpdatesAndSignalFences();

  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_EQ(registered_presents.size(), 0ul);

  EXPECT_EQ(GetUberStruct(flatland.get()), nullptr);

  EXPECT_FALSE(utils::IsEventSignalled(release1_copy, ZX_EVENT_SIGNALED));
  EXPECT_FALSE(utils::IsEventSignalled(release2_copy, ZX_EVENT_SIGNALED));

  // Signal the fence for the first Present(). This should trigger both Presents(), resulting no
  // registered Presents and an UberStruct with a 2-element topology: the local root, and kId.
  acquire1_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _)).Times(2);
  RunLoopUntilIdle();

  ApplySessionUpdatesAndSignalFences();

  registered_presents = GetRegisteredPresents(flatland->GetRoot().GetInstanceId());
  EXPECT_TRUE(registered_presents.empty());

  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_NE(uber_struct, nullptr);
  EXPECT_EQ(uber_struct->local_topology.size(), 2ul);

  EXPECT_TRUE(utils::IsEventSignalled(release1_copy, ZX_EVENT_SIGNALED));
  EXPECT_TRUE(utils::IsEventSignalled(release2_copy, ZX_EVENT_SIGNALED));
}

TEST_F(FlatlandTest, SetHitRegionsErrorTest) {
  // Zero is not a valid transform ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetHitRegions(kInvalidTransformId, {});
    Present(flatland, /*expect_success=*/false);
  }

  // Negative hit region width.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    const TransformId kId(1);
    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    fuchsia_ui_composition::wire::HitRegion region = {
        {0, 2, -10, 4}, fuchsia_ui_composition::HitTestInteraction::kDefault};
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/false);
  }

  // Transform ID should be present.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    const TransformId kId(42);
    flatland->SetHitRegions(kId, {});
    Present(flatland, /*expect_success=*/false);
  }

  auto interaction = fuchsia_ui_composition::HitTestInteraction::kDefault;
  const TransformId kId(1);

  // Height should be non-negative.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fuchsia_math::wire::RectF rect = {0, 0, 0, -1};
    fuchsia_ui_composition::wire::HitRegion region = {rect, interaction};

    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/false);
  }

  // Width should be non-negative.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fuchsia_math::wire::RectF rect = {0, 0, -1, 0};
    fuchsia_ui_composition::wire::HitRegion region = {rect, interaction};

    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/false);
  }

  // Negative origin should succeed.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fuchsia_math::wire::RectF rect = {-1, -1, 0, 0};
    fuchsia_ui_composition::wire::HitRegion region = {rect, interaction};

    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/true);
  }

  // Empty hit region vector should succeed.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {});
    Present(flatland, /*expect_success=*/true);
  }

  // Valid hit region vector should succeed.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fuchsia_math::wire::RectF rect = {0, 0, 10, 10};
    fuchsia_ui_composition::wire::HitRegion region = {rect, interaction};

    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/true);
  }

  // Consecutive SetRootTransforms with the same transform should work.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fuchsia_math::wire::RectF rect = {0, 0, 0, 0};
    fuchsia_ui_composition::wire::HitRegion region = {rect, interaction};

    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetHitRegions(kId, {region});
    Present(flatland, /*expect_success=*/true);
  }
}

TEST_F(FlatlandTest, SetDebugNameAddsPrefixToLogs) {
  // No prefix in errors by default.
  {
    std::optional<std::string> error_log;
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
    flatland->CreateTransform(kInvalidTransformId);
    Present(flatland, false);
    ASSERT_TRUE(error_log.has_value());
    EXPECT_EQ("CreateTransform called with transform_id=0", *error_log);
  }

  // SetDebugName() adds a prefix.
  {
    std::optional<std::string> error_log;
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
    flatland->SetDebugName("test_client");
    flatland->CreateTransform(kInvalidTransformId);
    Present(flatland, false);
    ASSERT_TRUE(error_log.has_value());
    EXPECT_EQ("Flatland client(test_client): CreateTransform called with transform_id=0",
              *error_log);
  }

  // ErrorReporter logs the prefix of multiple flatland clients correctly.
  {
    std::optional<std::string> error_log_a;
    std::optional<std::string> error_log_b;

    std::shared_ptr<Flatland> flatland_a = CreateFlatland();
    std::shared_ptr<Flatland> flatland_b = CreateFlatland();

    flatland_a->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log_a));
    flatland_b->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log_b));

    flatland_a->SetDebugName("test_client");
    flatland_b->SetDebugName("test_client1");

    flatland_a->CreateTransform(kInvalidTransformId);
    flatland_b->CreateTransform(kInvalidTransformId);

    Present(flatland_a, false);
    Present(flatland_b, false);

    ASSERT_TRUE(error_log_a.has_value());
    ASSERT_TRUE(error_log_b.has_value());

    EXPECT_EQ("Flatland client(test_client): CreateTransform called with transform_id=0",
              *error_log_a);
    EXPECT_EQ("Flatland client(test_client1): CreateTransform called with transform_id=0",
              *error_log_b);
  }
}

TEST_F(FlatlandTest, SetDebugNameAddsDebugNameToUberStruct) {
  // debug_name is empty when SetDebugName() is not called.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    Present(flatland, true);
    auto uber_struct = GetUberStruct(flatland.get());
    EXPECT_TRUE(uber_struct->debug_name.empty());
  }
  // UberStruct sees value of SetDebugName() after Present().
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetDebugName("test_client");
    Present(flatland, true);
    auto uber_struct = GetUberStruct(flatland.get());
    EXPECT_EQ("test_client", uber_struct->debug_name);
  }
  // Clear() resets the debug_name member of UberStruct.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetDebugName("test_client");
    Present(flatland, true);
    auto uber_struct = GetUberStruct(flatland.get());
    EXPECT_EQ("test_client", uber_struct->debug_name);
    flatland->Clear();
    Present(flatland, true);
    uber_struct = GetUberStruct(flatland.get());
    EXPECT_TRUE(uber_struct->debug_name.empty());
  }
}

TEST_F(FlatlandTest, CreateAndReleaseTransformValidCases) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const TransformId kId1(1);
  const TransformId kId2(2);

  // Create two transforms.
  flatland->CreateTransform(kId1);
  flatland->CreateTransform(kId2);
  Present(flatland, true);

  // Clear, then create two transforms in the other order.
  flatland->Clear();
  flatland->CreateTransform(kId2);
  flatland->CreateTransform(kId1);
  Present(flatland, true);

  // Clear, create and release transforms, non-overlapping.
  flatland->Clear();
  flatland->CreateTransform(kId1);
  flatland->ReleaseTransform(kId1);
  flatland->CreateTransform(kId2);
  flatland->ReleaseTransform(kId2);
  Present(flatland, true);

  // Clear, create and release transforms, nested.
  flatland->Clear();
  flatland->CreateTransform(kId2);
  flatland->CreateTransform(kId1);
  flatland->ReleaseTransform(kId1);
  flatland->ReleaseTransform(kId2);
  Present(flatland, true);

  // Reuse the same id, legally, in a single present call.
  flatland->CreateTransform(kId1);
  flatland->ReleaseTransform(kId1);
  flatland->CreateTransform(kId1);
  flatland->Clear();
  flatland->CreateTransform(kId1);
  Present(flatland, true);

  // Create and clear, overlapping, with multiple present calls.
  flatland->Clear();
  flatland->CreateTransform(kId2);
  Present(flatland, true);
  flatland->CreateTransform(kId1);
  flatland->ReleaseTransform(kId2);
  Present(flatland, true);
  flatland->ReleaseTransform(kId1);
  Present(flatland, true);
}

TEST_F(FlatlandTest, CreateAndReleaseTransformErrorCases) {
  const TransformId kId1(1);
  const TransformId kId2(2);

  // Zero is not a valid transform id.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kInvalidTransformId);
    Present(flatland, false);
  }
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->ReleaseTransform(kInvalidTransformId);
    Present(flatland, false);
  }

  // Double creation is an error.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kId1);
    flatland->CreateTransform(kId1);
    Present(flatland, false);
  }

  // Releasing a non-existent transform is an error.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->ReleaseTransform(kId2);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, AddAndRemoveChildValidCases) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const TransformId kIdParent(1);
  const TransformId kIdChild1(2);
  const TransformId kIdChild2(3);
  const TransformId kIdGrandchild(4);

  flatland->CreateTransform(kIdParent);
  flatland->CreateTransform(kIdChild1);
  flatland->CreateTransform(kIdChild2);
  flatland->CreateTransform(kIdGrandchild);
  Present(flatland, true);

  // Add and remove.
  flatland->AddChild(kIdParent, kIdChild1);
  flatland->RemoveChild(kIdParent, kIdChild1);
  Present(flatland, true);

  // Add two children.
  flatland->AddChild(kIdParent, kIdChild1);
  flatland->AddChild(kIdParent, kIdChild2);
  Present(flatland, true);

  // Remove two children.
  flatland->RemoveChild(kIdParent, kIdChild1);
  flatland->RemoveChild(kIdParent, kIdChild2);
  Present(flatland, true);

  // Add two-deep hierarchy.
  flatland->AddChild(kIdParent, kIdChild1);
  flatland->AddChild(kIdChild1, kIdGrandchild);
  Present(flatland, true);

  // Add sibling.
  flatland->AddChild(kIdParent, kIdChild2);
  Present(flatland, true);

  // Add shared grandchild (deadly diamond dependency).
  flatland->AddChild(kIdChild2, kIdGrandchild);
  Present(flatland, true);

  // Remove original deep-hierarchy.
  flatland->RemoveChild(kIdChild1, kIdGrandchild);
  Present(flatland, true);
}

TEST_F(FlatlandTest, AddAndRemoveChildErrorCases) {
  const TransformId kIdParent(1);
  const TransformId kIdChild(2);
  const TransformId kIdNotCreated(3);

  // Setup.
  auto SetupFlatland = [&]() {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kIdParent);
    flatland->CreateTransform(kIdChild);
    flatland->AddChild(kIdParent, kIdChild);
    return flatland;
  };

  {
    auto flatland = SetupFlatland();
    Present(flatland, true);
  }

  // Zero is not a valid transform id.
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kInvalidTransformId, kInvalidTransformId);
    Present(flatland, false);
  }
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kIdParent, kInvalidTransformId);
    Present(flatland, false);
  }
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kInvalidTransformId, kIdChild);
    Present(flatland, false);
  }

  // Child does not exist.
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kIdParent, kIdNotCreated);
    Present(flatland, false);
  }
  {
    auto flatland = SetupFlatland();
    flatland->RemoveChild(kIdParent, kIdNotCreated);
    Present(flatland, false);
  }

  // Parent does not exist.
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kIdNotCreated, kIdChild);
    Present(flatland, false);
  }
  {
    auto flatland = SetupFlatland();
    flatland->RemoveChild(kIdNotCreated, kIdChild);
    Present(flatland, false);
  }

  // Child is already a child of parent->
  {
    auto flatland = SetupFlatland();
    flatland->AddChild(kIdParent, kIdChild);
    Present(flatland, false);
  }

  // Both nodes exist, but not in the correct relationship.
  {
    auto flatland = SetupFlatland();
    flatland->RemoveChild(kIdChild, kIdParent);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, ReplaceChildren) {
  const TransformId kIdParent(1);
  const TransformId kIdChild1(2);
  const TransformId kIdChild2(3);
  const TransformId kIdChild3(4);
  const TransformId kIdChild4(5);

  // Setup.
  auto SetupFlatland = [&]() {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kIdParent);
    flatland->CreateTransform(kIdChild1);
    flatland->CreateTransform(kIdChild2);
    flatland->CreateTransform(kIdChild3);
    flatland->CreateTransform(kIdChild4);
    flatland->AddChild(kIdParent, kIdChild1);
    return flatland;
  };

  // ReplaceChildren fails with duplicate new children.
  {
    auto flatland = SetupFlatland();
    flatland->ReplaceChildren(kIdParent, {kIdChild2, kIdChild2, kIdChild3});
    Present(flatland, false);
  }

  {
    auto flatland = SetupFlatland();
    flatland->ReplaceChildren(kIdParent, {kIdChild2, kIdChild3, kIdChild4});
    Present(flatland, true);
  }
}

// Test that Transforms can be children to multiple different parents.
TEST_F(FlatlandTest, MultichildUsecase) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const TransformId kIdParent1(1);
  const TransformId kIdParent2(2);
  const TransformId kIdChild1(3);
  const TransformId kIdChild2(4);
  const TransformId kIdChild3(5);

  // Setup
  flatland->CreateTransform(kIdParent1);
  flatland->CreateTransform(kIdParent2);
  flatland->CreateTransform(kIdChild1);
  flatland->CreateTransform(kIdChild2);
  flatland->CreateTransform(kIdChild3);
  Present(flatland, true);

  // Add all children to first parent->
  flatland->AddChild(kIdParent1, kIdChild1);
  flatland->AddChild(kIdParent1, kIdChild2);
  flatland->AddChild(kIdParent1, kIdChild3);
  Present(flatland, true);

  // Add all children to second parent->
  flatland->AddChild(kIdParent2, kIdChild1);
  flatland->AddChild(kIdParent2, kIdChild2);
  flatland->AddChild(kIdParent2, kIdChild3);
  Present(flatland, true);
}

// Test that when a transform has multiple parents, that
// the number of nodes in the uber_structs global topology
// is what we would expect.
TEST_F(FlatlandTest, MultichildTest2) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const TransformId kIdRoot(1);
  const TransformId kIdParent1(2);
  const TransformId kIdParent2(3);
  const TransformId kIdChild(4);

  // Create the transforms.
  flatland->CreateTransform(kIdRoot);
  flatland->CreateTransform(kIdParent1);
  flatland->CreateTransform(kIdParent2);
  flatland->CreateTransform(kIdChild);
  Present(flatland, true);

  // Setup the diamond parent hierarchy.
  flatland->SetRootTransform(kIdRoot);
  flatland->AddChild(kIdRoot, kIdParent1);
  flatland->AddChild(kIdRoot, kIdParent2);
  flatland->AddChild(kIdParent1, kIdChild);
  flatland->AddChild(kIdParent2, kIdChild);
  Present(flatland, true);

  // The transform kIdChild should be doubly listed
  // in the uber struct.
  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_EQ(uber_struct->local_topology.size(), 6U);
}

// Test that Present() fails if it detects a graph cycle.
TEST_F(FlatlandTest, CycleDetector) {
  const TransformId kId1(1);
  const TransformId kId2(2);
  const TransformId kId3(3);
  const TransformId kId4(4);

  // Create an immediate cycle.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kId1);
    flatland->AddChild(kId1, kId1);
    Present(flatland, false);
  }

  // Create a legal chain of depth one.
  // Then, create a cycle of length 2.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->Clear();
    flatland->CreateTransform(kId1);
    flatland->CreateTransform(kId2);
    flatland->AddChild(kId1, kId2);
    Present(flatland, true);

    flatland->AddChild(kId2, kId1);
    Present(flatland, false);
  }

  // Create two legal chains of length one.
  // Then, connect each chain into a cycle of length four.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->Clear();
    flatland->CreateTransform(kId1);
    flatland->CreateTransform(kId2);
    flatland->CreateTransform(kId3);
    flatland->CreateTransform(kId4);
    flatland->AddChild(kId1, kId2);
    flatland->AddChild(kId3, kId4);
    Present(flatland, true);

    flatland->AddChild(kId2, kId3);
    flatland->AddChild(kId4, kId1);
    Present(flatland, false);
  }

  // Create a cycle, where the root is not involved in the cycle.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->Clear();
    flatland->CreateTransform(kId1);
    flatland->CreateTransform(kId2);
    flatland->CreateTransform(kId3);
    flatland->CreateTransform(kId4);

    flatland->AddChild(kId1, kId2);
    flatland->AddChild(kId2, kId3);
    flatland->AddChild(kId3, kId2);
    flatland->AddChild(kId3, kId4);

    flatland->SetRootTransform(kId1);
    flatland->ReleaseTransform(kId1);
    flatland->ReleaseTransform(kId2);
    flatland->ReleaseTransform(kId3);
    flatland->ReleaseTransform(kId4);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetRootTransform) {
  const TransformId kId1(1);
  const TransformId kIdNotCreated(2);

  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kId1);
    Present(flatland, true);

    // Even with no root transform, so clearing it is not an error.
    flatland->SetRootTransform(kInvalidTransformId);
    Present(flatland, true);

    flatland->SetRootTransform(kId1);
    Present(flatland, true);

    // Setting the root to a non-existent transform does not clear the root, which means the local
    // topology will contain two handles: the "local root" and kId1.
    auto uber_struct = GetUberStruct(flatland.get());
    EXPECT_EQ(uber_struct->local_topology.size(), 2ul);

    // Releasing the root is allowed, though it will remain in the hierarchy until reset.
    flatland->ReleaseTransform(kId1);
    Present(flatland, true);

    // Clearing the root after release is also allowed.
    flatland->SetRootTransform(kInvalidTransformId);
    Present(flatland, true);

    // Setting the root to a released transform is not allowed.
    flatland->SetRootTransform(kId1);
    Present(flatland, false);
  }

  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetRootTransform(kIdNotCreated);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetTranslationErrorCases) {
  const TransformId kIdNotCreated(1);

  // Zero is not a valid transform ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetTranslation(kInvalidTransformId, {1, 2});
    Present(flatland, false);
  }

  // Transform does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetTranslation(kIdNotCreated, {1, 2});
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetOrientationErrorCases) {
  const TransformId kIdNotCreated(1);

  // Zero is not a valid transform ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetOrientation(kInvalidTransformId, Orientation::kCcw90Degrees);
    Present(flatland, false);
  }

  // Transform does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetOrientation(kIdNotCreated, Orientation::kCcw90Degrees);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetScaleErrorCases) {
  const TransformId kIdNotCreated(1);

  // Zero is not a valid transform ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetScale(kInvalidTransformId, {1, 2});
    Present(flatland, false);
  }

  // Transform does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetScale(kIdNotCreated, {1, 2});
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetImageDestinationSizeErrorCases) {
  const ContentId kIdNotCreated(1);

  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageDestinationSize(kInvalidContentId, {1, 2});
    Present(flatland, false);
  }

  // Content does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageDestinationSize(kIdNotCreated, {1, 2});
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetImageBlendFunctionErrorCases) {
  const ContentId kIdNotCreated(1);

  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageBlendMode(kInvalidContentId, BlendMode::kReplace());
    Present(flatland, false);
  }

  // Content does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageBlendMode(kIdNotCreated, BlendMode::kReplace());
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetImageFlipErrorCases) {
  const ContentId kIdNotCreated(1);

  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageFlip(kInvalidContentId, fuchsia_ui_composition::ImageFlip::kLeftRight);
    Present(flatland, false);
  }

  // Content does not exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageFlip(kIdNotCreated, fuchsia_ui_composition::ImageFlip::kLeftRight);
    Present(flatland, false);
  }
}

// Test that changing geometric transform properties affects the local matrix of Transforms.
TEST_F(FlatlandTest, SetGeometricTransformProperties) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create two Transforms to ensure properties are local to individual Transforms.
  const TransformId kId1(1);
  const TransformId kId2(2);

  flatland->CreateTransform(kId1);
  flatland->CreateTransform(kId2);

  flatland->SetRootTransform(kId1);
  flatland->AddChild(kId1, kId2);

  Present(flatland, true);

  // Get the TransformHandles for kId1 and kId2.
  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_EQ(uber_struct->local_topology.size(), 3ul);
  ASSERT_EQ(uber_struct->local_topology[0].handle, flatland->GetRoot());

  const auto handle1 = uber_struct->local_topology[1].handle;
  const auto handle2 = uber_struct->local_topology[2].handle;

  // The local topology will always have 3 transforms: the local root, kId1, and kId2. With no
  // properties set, there will be no local matrices.
  uber_struct = GetUberStruct(flatland.get());
  EXPECT_TRUE(uber_struct->local_matrices.empty());

  // Set one transform property for each node.
  // Set translation on first transform.
  flatland->SetTranslation(kId1, {1, 2});

  // Set scale on second transform.
  flatland->SetScale(kId2, {2.f, 3.f});
  Present(flatland, true);

  // The two handles should have the expected matrices.
  uber_struct = GetUberStruct(flatland.get());
  ExpectMatrix(uber_struct, handle1, glm::translate(glm::mat3(), {1, 2}));
  ExpectMatrix(uber_struct, handle2, glm::scale(glm::mat3(), {2.f, 3.f}));

  // Fill out the remaining properties on both transforms.
  flatland->SetScale(kId1, {4.f, 5.f});
  flatland->SetOrientation(kId1, Orientation::kCcw90Degrees);

  flatland->SetOrientation(kId2, Orientation::kCcw270Degrees);
  flatland->SetTranslation(kId2, {6, 7});

  Present(flatland, true);

  // Verify the new properties were applied in the correct orders.
  uber_struct = GetUberStruct(flatland.get());

  // The way the glm function calls handle translation/scale/rotation is
  // a bit unintuitive. If you call glm::scale(translation_matrix, vec2),
  // this may appear as if we are incorrectly translating first and scaling
  // second, but glm will apply them in the order (translation * scale * (point))
  // which is indeed the ordering that we want. In other words, the built-in glm
  // calls *right multiply* instead of *left multiply*.
  glm::mat3 matrix1 = glm::mat3();
  matrix1 = glm::translate(matrix1, {1, 2});
  matrix1 = glm::rotate(matrix1, utils::GetOrientationAngle(Orientation::kCcw90Degrees));
  matrix1 = glm::scale(matrix1, {4.f, 5.f});
  ExpectMatrix(uber_struct, handle1, matrix1);

  glm::mat3 matrix2 = glm::mat3();
  matrix2 = glm::translate(matrix2, {6, 7});
  matrix2 = glm::rotate(matrix2, utils::GetOrientationAngle(Orientation::kCcw270Degrees));
  matrix2 = glm::scale(matrix2, {2.f, 3.f});
  ExpectMatrix(uber_struct, handle2, matrix2);
}

// Ensure that local matrix data is only cleaned up when a Transform is completely unreferenced,
// meaning no Transforms reference it as a child->
TEST_F(FlatlandTest, MatrixReleasesWhenTransformNotReferenced) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create two Transforms to ensure properties are local to individual Transforms.
  const TransformId kId1(1);
  const TransformId kId2(2);

  flatland->CreateTransform(kId1);
  flatland->CreateTransform(kId2);

  flatland->SetRootTransform(kId1);
  flatland->AddChild(kId1, kId2);

  Present(flatland, true);

  // Get the TransformHandles for kId1 and kId2.
  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_EQ(uber_struct->local_topology.size(), 3ul);
  ASSERT_EQ(uber_struct->local_topology[0].handle, flatland->GetRoot());

  const auto handle1 = uber_struct->local_topology[1].handle;
  const auto handle2 = uber_struct->local_topology[2].handle;

  // Set a geometric property on kId1.
  flatland->SetTranslation(kId1, {1, 2});
  Present(flatland, true);

  // Only handle1 should have a local matrix.
  uber_struct = GetUberStruct(flatland.get());
  ExpectMatrix(uber_struct, handle1, glm::translate(glm::mat3(), {1, 2}));

  // Release kId1, but ensure its matrix stays around.
  flatland->ReleaseTransform(kId1);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  ExpectMatrix(uber_struct, handle1, glm::translate(glm::mat3(), {1, 2}));

  // Clear kId1 as the root transform, which should clear the matrix.
  flatland->SetRootTransform(kInvalidTransformId);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  EXPECT_TRUE(uber_struct->local_matrices.empty());
}

TEST_F(FlatlandTest, CreateViewReplaceWithoutConnection) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  flatland->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  Present(flatland, true);

  ViewportCreationToken parent_token2;
  ViewCreationToken child_token2;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token2.value(), &child_token2.value()));

  auto [parent_viewport_watcher_client_end2, parent_viewport_watcher_server_end2] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  flatland->CreateView2(ToWire(std::move(child_token2)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end2));

  RunLoopUntilIdle();

  // Until Present() is called, the previous ParentViewportWatcher is not unbound.
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));

  Present(flatland, true);

  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));
}

TEST_F(FlatlandTest, ParentViewportWatcherReplaceWithConnection) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  CreateViewport(parent.get(), child.get(), kLinkId1, std::move(child_view_watcher_server_end),
                 std::move(parent_viewport_watcher_server_end));

  // Don't use the helper function for the second link to test when the previous links are closed.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Creating the new ParentViewportWatcher doesn't invalidate either of the old links until
  // Present() is called on the child->
  auto [parent_viewport_watcher_client_end2, parent_viewport_watcher_server_end2] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end2));

  RunLoopUntilIdle();

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));

  // Present() replaces the original ParentViewportWatcher, which also results in the invalidation
  // of both ends of the original link.
  Present(child, true);

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));
}

TEST_F(FlatlandTest, ParentViewportWatcherUnbindsOnParentDeath) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  flatland->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  Present(flatland, true);

  parent_token.value().reset();
  RunLoopUntilIdle();

  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end));
}

TEST_F(FlatlandTest, ParentViewportWatcherUnbindsImmediatelyWithInvalidToken) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewCreationToken child_token;

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  flatland->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  // The link will be unbound even before Present() is called.
  RunLoopUntilIdle();
  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end));

  Present(flatland, false);
}

TEST_F(FlatlandTest, ReleaseViewFailsWithoutLink) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  flatland->ReleaseView();

  Present(flatland, false);
}

TEST_F(FlatlandTest, ReleaseViewSucceedsWithLink) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  flatland->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  Present(flatland, true);

  // Killing the peer token does not prevent the instance from releasing view.
  parent_token.value().reset();
  RunLoopUntilIdle();

  flatland->ReleaseView();
  Present(flatland, true);
}

TEST_F(FlatlandTest, CreateViewSuccceedsAfterReleaseView) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  flatland->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  Present(flatland, true);

  // Killing the peer token does not prevent the instance from releasing view.
  parent_token.value().reset();
  RunLoopUntilIdle();

  ViewportCreationToken parent_token2;
  ViewCreationToken child_token2;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token2.value(), &child_token2.value()));

  auto [parent_viewport_watcher_client_end2, parent_viewport_watcher_server_end2] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  flatland->CreateView2(ToWire(std::move(child_token2)), NewWireViewIdentityOnCreation(),
                        NoViewProtocols(), std::move(parent_viewport_watcher_server_end2));
  Present(flatland, true);
}

TEST_F(FlatlandTest, RegisterViewBoundProtocols_BothTouchSources_ReturnsBadOperation) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto creation_tokens = scenic::cpp::ViewCreationTokenPair::New();

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  auto [touch_source_client_end, touch_source_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();
  auto [touch_source_v2_client_end, touch_source_v2_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::TouchSourceV2>::Create();

  fidl::Arena arena;
  auto protocols = fuchsia_ui_composition::wire::ViewBoundProtocols::Builder(arena)
                       .touch_source(std::move(touch_source_server_end))
                       .touch_source_v2(std::move(touch_source_v2_server_end))
                       .Build();

  flatland->CreateView2(ToWire(std::move(creation_tokens.view_token)),
                        NewWireViewIdentityOnCreation(), protocols,
                        std::move(parent_viewport_watcher_server_end));

  RunLoopUntilIdle();

  EXPECT_EQ(GetFlatlandError(flatland->GetSessionId()), FlatlandError::kBadOperation);
}

TEST_F(FlatlandTest, RegisterViewBoundProtocols_BothMouseSources_ReturnsBadOperation) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto creation_tokens = scenic::cpp::ViewCreationTokenPair::New();

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  auto [mouse_source_client_end, mouse_source_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::MouseSource>::Create();
  auto [mouse_source_v2_client_end, mouse_source_v2_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::MouseSourceV2>::Create();

  fidl::Arena arena;
  auto protocols = fuchsia_ui_composition::wire::ViewBoundProtocols::Builder(arena)
                       .mouse_source(std::move(mouse_source_server_end))
                       .mouse_source_v2(std::move(mouse_source_v2_server_end))
                       .Build();

  flatland->CreateView2(ToWire(std::move(creation_tokens.view_token)),
                        NewWireViewIdentityOnCreation(), protocols,
                        std::move(parent_viewport_watcher_server_end));

  RunLoopUntilIdle();

  EXPECT_EQ(GetFlatlandError(flatland->GetSessionId()), FlatlandError::kBadOperation);
}

TEST_F(FlatlandTest, ChildViewWatcherUnbindsOnChildDeath) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, true);

  child_token.value().reset();
  RunLoopUntilIdle();

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end));
}

TEST_F(FlatlandTest, ChildViewWatcherUnbindsImmediatelyWithInvalidToken) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), {},
                           std::move(child_view_watcher_server_end));

  // The link will be unbound even before Present() is called.
  RunLoopUntilIdle();
  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end));

  Present(flatland, false);
}

TEST_F(FlatlandTest, ChildViewWatcherFailsIdIsZero) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  flatland->CreateViewport(kInvalidContentId, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, false);
}

TEST_F(FlatlandTest, ChildViewWatcherFailsNoLogicalSize) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena).Build();
  flatland->CreateViewport(kInvalidContentId, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, false);
}

TEST_F(FlatlandTest, ChildViewWatcherFailsInvalidLogicalSize) {
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;

  // The X value must be positive.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{0, kDefaultSize})
                          .Build();
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    flatland->CreateViewport(kInvalidContentId, ToWire(std::move(parent_token)), properties,
                             std::move(child_view_watcher_server_end));
    Present(flatland, false);
  }

  // The Y value must be positive.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));
    fidl::Arena arena;
    auto properties2 = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                           .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, 0})
                           .Build();
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    flatland->CreateViewport(kInvalidContentId, ToWire(std::move(parent_token)), properties2,
                             std::move(child_view_watcher_server_end));
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, ChildViewAutomaticallyClipsBounds) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId1(1);
  // Create the viewport and check the uberstruct for the clip bounds.
  {
    const int32_t kWidth = 300;
    const int32_t kHeight = 500;
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kWidth, kHeight})
                          .Build();
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                             std::move(child_view_watcher_server_end));
    Present(flatland, true);

    auto maybe_transform = flatland->GetContentHandle(kLinkId1);
    EXPECT_TRUE(maybe_transform);
    auto transform = *maybe_transform;

    auto uber_struct = GetUberStruct(flatland.get());
    auto clip_itr = uber_struct->local_clip_regions.find(transform);
    EXPECT_NE(clip_itr, uber_struct->local_clip_regions.end());

    auto clip_region = clip_itr->second;
    EXPECT_EQ(clip_region,
              TransformClipRegion({.x = 0, .y = 0, .width = kWidth, .height = kHeight}));
  }

  // Change the bounds via a call to |SetViewProperties| and make sure they've changed.
  {
    const int32_t kWidth = 900;
    const int32_t kHeight = 700;
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kWidth, kHeight})
                          .Build();
    flatland->SetViewportProperties(kLinkId1, properties);
    Present(flatland, true);

    auto maybe_transform = flatland->GetContentHandle(kLinkId1);
    EXPECT_TRUE(maybe_transform);
    auto transform = *maybe_transform;

    auto uber_struct = GetUberStruct(flatland.get());
    auto clip_itr = uber_struct->local_clip_regions.find(transform);
    EXPECT_NE(clip_itr, uber_struct->local_clip_regions.end());

    auto clip_region = clip_itr->second;
    EXPECT_EQ(clip_region,
              TransformClipRegion({.x = 0, .y = 0, .width = kWidth, .height = kHeight}));
  }
}

TEST_F(FlatlandTest, ViewportClippingPersistsAcrossInstances) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Create and link the two instances.
  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);

  const ContentId kLinkId(1);

  const int32_t kViewportWidth = 75;
  const int32_t kViewportHeight = 325;
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kViewportWidth, kViewportHeight})
                        .Build();
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kId1, kLinkId);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  Present(parent, true);
  Present(child, true);

  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  // Calculate the global clip regions of the parent and child flatland instances, and ensure
  // that the root of the child instance has a global clip region equivalent to that of the
  // logical size of the parent viewport's properties.
  const auto snapshot = uber_struct_system_->Snapshot();
  const auto links = link_system_->GetResolvedTopologyLinks();
  const auto link_system_id = link_system_->GetInstanceId();

  const auto topology_data = flatland::GlobalTopologyData::ComputeGlobalTopologyData(
      snapshot.map, links, link_system_id, parent->GetRoot());
  const auto global_matrices = flatland::ComputeGlobalMatrices(
      topology_data.topology_vector, topology_data.parent_indices, snapshot.map);

  const auto global_clip_regions = ComputeGlobalTransformClipRegions(
      topology_data.topology_vector, topology_data.parent_indices, global_matrices, snapshot.map);

  auto child_root_handle = topology_data.topology_vector[3];
  EXPECT_EQ(child->GetRoot(), child_root_handle);
  auto child_root_clip = global_clip_regions[3];
  EXPECT_EQ(
      child_root_clip,
      TransformClipRegion({.x = 0, .y = 0, .width = kViewportWidth, .height = kViewportHeight}));
}

TEST_F(FlatlandTest, DefaultHitRegion_IsInfinite) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  flatland->CreateTransform(kId1);
  flatland->SetRootTransform(kId1);

  Present(flatland, true);
  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    ASSERT_EQ(hit_regions.size(), 1u);
    ASSERT_EQ(hit_regions.at(handle1).size(), 1u);
    EXPECT_FALSE(hit_regions.at(handle1)[0].is_finite());
  }
}

TEST_F(FlatlandTest, DefaultHitRegionsExist_OnlyForCurrentRoot) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  const TransformId kId2(2);
  const TransformHandle handle2 = TransformHandle(session_id, 2);

  flatland->CreateTransform(kId1);
  flatland->CreateTransform(kId2);

  flatland->SetRootTransform(kId1);

  Present(flatland, true);
  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 1u);
    EXPECT_EQ(hit_regions.at(handle1).size(), 1u);
    EXPECT_FALSE(hit_regions.contains(handle2));
  }

  flatland->SetRootTransform(kId2);

  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 1u);
    EXPECT_FALSE(hit_regions.contains(handle1));
    EXPECT_EQ(hit_regions.at(handle2).size(), 1u);
  }
}

TEST_F(FlatlandTest, SetHitRegionsOverwritesPreviousOnes) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  flatland->CreateTransform(kId1);
  flatland->SetRootTransform(kId1);

  Present(flatland, true);

  // Check that the default hit region is as expected.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 1u);
    EXPECT_EQ(hit_regions.at(handle1).size(), 1u);

    auto hit_region = hit_regions.at(handle1)[0];

    EXPECT_FALSE(hit_region.is_finite());
    EXPECT_EQ(hit_region.interaction(), fuchsia_ui_composition::HitTestInteraction::kDefault);
  }

  // Add a hit region to a different transform - this should not overwrite the default one.
  const TransformId kId2(2);
  const TransformHandle handle2 = TransformHandle(session_id, 2);

  flatland->CreateTransform(kId2);
  flatland->SetHitRegions(kId2,
                          {{{0, 1, 2, 3}, fuchsia_ui_composition::HitTestInteraction::kDefault}});

  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 2u);
    EXPECT_EQ(hit_regions.at(handle1).size(), 1u);
    EXPECT_EQ(hit_regions.at(handle2).size(), 1u);

    {
      auto hit_region = hit_regions.at(handle1)[0];

      EXPECT_FALSE(hit_region.is_finite());
      EXPECT_EQ(hit_region.interaction(), fuchsia_ui_composition::HitTestInteraction::kDefault);
    }

    {
      auto hit_region = hit_regions.at(handle2)[0];

      ASSERT_TRUE(hit_region.is_finite());
      const auto rect = hit_region.region();
      const types::RectangleF expected_rect({.x = 0, .y = 1, .width = 2, .height = 3});
      EXPECT_EQ(rect, expected_rect);
      EXPECT_EQ(hit_region.interaction(), fuchsia_ui_composition::HitTestInteraction::kDefault);
    }
  }

  // Overwrite the default hit region.
  flatland->SetHitRegions(
      kId1, {{{1, 2, 3, 4}, fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible}});

  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 2u);
    EXPECT_EQ(hit_regions.at(handle1).size(), 1u);

    auto hit_region = hit_regions.at(handle1)[0];

    ASSERT_TRUE(hit_region.is_finite());
    const auto rect = hit_region.region();
    const types::RectangleF expected_rect({.x = 1, .y = 2, .width = 3, .height = 4});
    EXPECT_EQ(rect, expected_rect);
    EXPECT_EQ(hit_region.interaction(),
              fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible);
  }
}

TEST_F(FlatlandTest, SetRootTransformAfterSetHitRegions_DoesNotChangeHitRegion) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  flatland->CreateTransform(kId1);
  flatland->SetHitRegions(
      kId1, {{{0, 1, 2, 3}, fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible}});
  flatland->SetRootTransform(kId1);

  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  auto& hit_regions = uber_struct->local_hit_regions_map;

  EXPECT_EQ(hit_regions.size(), 1u);
  EXPECT_EQ(hit_regions.at(handle1).size(), 1u);

  auto hit_region = hit_regions.at(handle1)[0];

  ASSERT_TRUE(hit_region.is_finite());
  const auto rect = hit_region.region();
  const types::RectangleF expected_rect({.x = 0, .y = 1, .width = 2, .height = 3});
  EXPECT_EQ(rect, expected_rect);
  EXPECT_EQ(hit_region.interaction(),
            fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible);
}

TEST_F(FlatlandTest, MultipleTransformsWithHitRegions) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  flatland->CreateTransform(kId1);
  flatland->SetHitRegions(kId1,
                          {{{0, 1, 2, 3}, fuchsia_ui_composition::HitTestInteraction::kDefault}});

  const TransformId kId2(2);
  const TransformHandle handle2 = TransformHandle(session_id, 2);

  flatland->CreateTransform(kId2);
  flatland->SetHitRegions(
      kId2, {{{1, 2, 3, 4}, fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible}});

  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    auto& hit_regions = uber_struct->local_hit_regions_map;

    EXPECT_EQ(hit_regions.size(), 2u);
    EXPECT_EQ(hit_regions.at(handle1).size(), 1u);
    EXPECT_EQ(hit_regions.at(handle2).size(), 1u);

    auto hit_region1 = hit_regions.at(handle1)[0];
    auto hit_region2 = hit_regions.at(handle2)[0];

    ASSERT_TRUE(hit_region1.is_finite());
    auto rect1 = hit_region1.region();
    ASSERT_TRUE(hit_region2.is_finite());
    auto rect2 = hit_region2.region();
    const types::RectangleF expected_rect1({.x = 0, .y = 1, .width = 2, .height = 3});
    const types::RectangleF expected_rect2({.x = 1, .y = 2, .width = 3, .height = 4});
    EXPECT_EQ(rect1, expected_rect1);
    EXPECT_EQ(rect2, expected_rect2);

    EXPECT_EQ(hit_region1.interaction(), fuchsia_ui_composition::HitTestInteraction::kDefault);
    EXPECT_EQ(hit_region2.interaction(),
              fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible);
  }
}

TEST_F(FlatlandTest, ManuallyAddedMaximalHitRegionPersists) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const auto session_id = flatland->GetSessionId();

  const TransformId kId1(1);
  const TransformHandle handle1 = TransformHandle(session_id, 1);

  flatland->CreateTransform(kId1);
  flatland->SetHitRegions(kId1, {{{FLT_MIN, FLT_MIN, FLT_MAX, FLT_MAX},
                                  fuchsia_ui_composition::HitTestInteraction::kDefault}});
  Present(flatland, true);

  flatland->SetRootTransform(kId1);
  Present(flatland, true);

  flatland->SetRootTransform(kInvalidTransformId);
  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  auto& hit_regions = uber_struct->local_hit_regions_map;

  EXPECT_EQ(hit_regions.size(), 1u);
  EXPECT_EQ(hit_regions.at(handle1).size(), 1u);
  auto hit_region1 = hit_regions.at(handle1)[0];

  ASSERT_TRUE(hit_region1.is_finite());
  auto rect = hit_region1.region();
  types::RectangleF expected_rect(
      {.x = FLT_MIN, .y = FLT_MIN, .width = FLT_MAX, .height = FLT_MAX});
  EXPECT_EQ(rect, expected_rect);
}

TEST_F(FlatlandTest, ChildViewWatcherFailsIdCollision) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  flatland->CreateViewport(kId1, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, true);

  ViewportCreationToken parent_token2;
  ViewCreationToken child_token2;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token2.value(), &child_token2.value()));

  flatland->CreateViewport(kId1, ToWire(std::move(parent_token2)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, false);
}

TEST_F(FlatlandTest, ClearDelaysLinkDestructionUntilPresent) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  CreateViewport(parent.get(), child.get(), kLinkId1, std::move(child_view_watcher_server_end),
                 std::move(parent_viewport_watcher_server_end));

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end));

  // Clearing the parent graph should not unbind the interfaces until Present() is called and the
  // acquire fence is signaled.
  parent->Clear();
  RunLoopUntilIdle();

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end));

  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(1);
  auto event_copy = utils::CopyZxHandle(args.acquire_fences[0]);

  PresentWithArgs(parent, std::move(args), true);

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end));

  // Signal the acquire fence to unbind the links.
  event_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end));

  // Recreate the Link. The parent graph was cleared so we can reuse the LinkId.
  auto [child_view_watcher_client_end2, child_view_watcher_server_end2] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  auto [parent_viewport_watcher_client_end2, parent_viewport_watcher_server_end2] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  CreateViewport(parent.get(), child.get(), kLinkId1, std::move(child_view_watcher_server_end2),
                 std::move(parent_viewport_watcher_server_end2));

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end2));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));

  // Clearing the child graph should not unbind the interfaces until Present() is called and the
  // acquire fence is signaled.
  child->Clear();
  RunLoopUntilIdle();

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end2));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));

  PresentArgs args2;
  args2.acquire_fences = utils::CreateEventArray(1);
  event_copy = utils::CopyZxHandle(args2.acquire_fences[0]);

  PresentWithArgs(child, std::move(args2), true);

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end2));
  EXPECT_TRUE(ClientEndPeerExists(parent_viewport_watcher_client_end2));

  // Signal the acquire fence to unbind the links.
  event_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end2));
  EXPECT_FALSE(ClientEndPeerExists(parent_viewport_watcher_client_end2));
}

// This test doesn't use the helper function to create a link, because it tests intermediate steps
// and timing corner cases.
TEST_F(FlatlandTest, ChildGetsLayoutUpdateWithoutPresenting) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Set up a link, but don't call Present() on either instance.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  // Request a layout update.
  std::optional<LayoutInfo> layout;
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetLayoutResult& result) {
        ASSERT_TRUE(result.is_ok());
        layout = std::move(result->info());
      });

  // Without even presenting, the child is able to get the initial properties from the parent->
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(layout.has_value());
  EXPECT_EQ(1u, layout->logical_size()->width());
  EXPECT_EQ(2u, layout->logical_size()->height());
}

TEST_F(FlatlandTest, OverwrittenHangingGetsReturnError) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Set up a link, but don't call Present() on either instance.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId(1);
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  UpdateLinks(parent->GetRoot());

  // First layout request should succeed immediately.
  bool layout_updated = false;
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetLayoutResult& result) {
        ASSERT_TRUE(result.is_ok());
        layout_updated = true;
      });
  RunLoopUntilIdle();
  EXPECT_TRUE(layout_updated);

  // Queue overwriting hanging gets.
  layout_updated = false;
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetLayoutResult& result) {
        if (result.is_ok()) {
          layout_updated = true;
        }
      });
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetLayoutResult& result) {
        if (result.is_ok()) {
          layout_updated = true;
        }
      });
  RunLoopUntilIdle();
  EXPECT_FALSE(layout_updated);

  // Present should fail on child because the client has broken flow control.
  PresentArgs args;
  args.expected_error = FlatlandError::kBadHangingGet;
  PresentWithArgs(child, std::move(args), false);
}

// This test doesn't use the helper function to create a link, because it tests intermediate steps
// and timing corner cases.
TEST_F(FlatlandTest, ConnectedToDisplayParentPresentsBeforeChild) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Set up a link and attach it to the parent's root, but don't call Present() on either instance.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const TransformId kTransformId(1);

  parent->CreateTransform(kTransformId);
  parent->SetRootTransform(kTransformId);

  const ContentId kLinkId(2);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kTransformId, kLinkId);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  // Request a status update.
  std::optional<ParentViewportStatus> parent_status;
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  // The child begins disconnected from the display.
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kDisconnectedFromDisplay);

  // The ParentViewportStatus will update when both the parent and child Present().
  parent_status.reset();
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  // The parent presents first, no update.
  Present(parent, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_FALSE(parent_status.has_value());

  // The child presents second and the status updates.
  Present(child, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kConnectedToDisplay);
}

// This test doesn't use the helper function to create a link, because it tests intermediate steps
// and timing corner cases.
TEST_F(FlatlandTest, ConnectedToDisplayChildPresentsBeforeParent) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Set up a link and attach it to the parent's root, but don't call Present() on either instance.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const TransformId kTransformId(1);

  parent->CreateTransform(kTransformId);
  parent->SetRootTransform(kTransformId);

  const ContentId kLinkId(2);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kTransformId, kLinkId);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  // Request a status update.
  std::optional<ParentViewportStatus> parent_status;
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  // The child begins disconnected from the display.
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kDisconnectedFromDisplay);

  // The ParentViewportStatus will update when both the parent and child Present().
  parent_status.reset();
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  // The child presents first, no update.
  Present(child, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_FALSE(parent_status.has_value());

  // The parent presents second and the status updates.
  Present(parent, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kConnectedToDisplay);
}

// This test doesn't use the helper function to create a link, because it tests intermediate steps
// and timing corner cases.
TEST_F(FlatlandTest, ChildReceivesDisconnectedFromDisplay) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Set up a link and attach it to the parent's root, but don't call Present() on either instance.
  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const TransformId kTransformId(1);

  parent->CreateTransform(kTransformId);
  parent->SetRootTransform(kTransformId);

  const ContentId kLinkId(2);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kTransformId, kLinkId);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  // The ParentViewportStatus will update when both the parent and child Present().
  std::optional<ParentViewportStatus> parent_status;
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  Present(child, true);
  Present(parent, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kConnectedToDisplay);

  // The ParentViewportStatus will update again if the parent removes the child link from its
  // topology.
  parent_status.reset();
  parent_viewport_watcher->GetStatus().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_status = result->status();
      });

  parent->SetContent(kTransformId, kInvalidContentId);
  Present(parent, true);

  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(parent_status.has_value());
  EXPECT_EQ(*parent_status, ParentViewportStatus::kDisconnectedFromDisplay);
}

// This test doesn't use the helper function to create a link, because it tests intermediate steps
// and timing corner cases.
TEST_F(FlatlandTest, ValidChildToParentFlow_ChildUsedCreateView2) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_viewport_token;
  ViewCreationToken child_view_token;
  ASSERT_EQ(ZX_OK,
            zx::channel::create(0, &parent_viewport_token.value(), &child_view_token.value()));

  const ContentId kLinkId(1);
  const TransformId kRootTransform(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Client child_view_watcher(std::move(child_view_watcher_client_end), dispatcher());

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_viewport_token)), properties,
                         std::move(child_view_watcher_server_end));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_view_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  std::optional<ChildViewStatus> child_status;
  child_view_watcher->GetStatus().ThenExactlyOnce([&](ChildViewWatcher_GetStatusResult& result) {
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(ChildViewStatus::kContentHasPresented, result->status());
    child_status = result->status();
  });

  std::optional<fuchsia_ui_views::ViewRef> child_viewref;
  child_view_watcher->GetViewRef().ThenExactlyOnce([&](ChildViewWatcher_GetViewRefResult& result) {
    ASSERT_TRUE(result.is_ok());
    child_viewref = std::move(result->view_ref());
  });

  // ChildViewStatus changes as soon as the child presents. The parent does not have to present.
  EXPECT_FALSE(child_status.has_value());

  Present(child, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(child_status.has_value());

  // Note that although CONTENT_HAS_PRESENTED is signaled, GetViewRef() does not yet return the ref.
  // This is because although the parent and child are connected, neither appears in the global
  // topology, because neither is connected to the root.
  EXPECT_FALSE(child_viewref.has_value());

  // Having the parent present is still not sufficient to fulfill GetViewRef(), because the viewport
  // has not been added to the parent's local sub-tree, and therefore doesn't appear in the global
  // topology.
  Present(parent, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_FALSE(child_viewref.has_value());

  // Adding the viewport to the parent's local tree, then presenting, is sufficient for the call to
  // GetViewRef() to succeed.
  parent->CreateTransform(kRootTransform);
  parent->SetRootTransform(kRootTransform);
  parent->SetContent(kRootTransform, kLinkId);

  // While we're at it, add an acquire fence and make sure that the ViewRef isn't sent until the
  // event is signaled.
  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(1);
  auto event_copy = utils::CopyZxHandle(args.acquire_fences[0]);

  // We still don't get the ViewRef, because we haven't signaled the event.
  PresentWithArgs(parent, std::move(args), true);
  UpdateLinks(parent->GetRoot());
  EXPECT_FALSE(child_viewref.has_value());

  // Signal the acquire fence to unblock the present.
  event_copy.signal(0, ZX_EVENT_SIGNALED);
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();
  ApplySessionUpdatesAndSignalFences();
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(child_viewref.has_value());
  EXPECT_TRUE(child_viewref->reference());
}

TEST_F(FlatlandTest, ValidChildToParentFlow_ChildUsedCreateView) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_viewport_token;
  ViewCreationToken child_view_token;
  ASSERT_EQ(ZX_OK,
            zx::channel::create(0, &parent_viewport_token.value(), &child_view_token.value()));

  const ContentId kLinkId(1);
  const TransformId kRootTransform(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Client child_view_watcher(std::move(child_view_watcher_client_end), dispatcher());

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_viewport_token)), properties,
                         std::move(child_view_watcher_server_end));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  child->CreateView(ToWire(std::move(child_view_token)),
                    std::move(parent_viewport_watcher_server_end));

  std::optional<ChildViewStatus> child_status;
  child_view_watcher->GetStatus().ThenExactlyOnce([&](ChildViewWatcher_GetStatusResult& result) {
    ASSERT_TRUE(result.is_ok());
    child_status = result->status();
  });

  // ChildViewStatus changes as soon as the child presents. The parent does not have to present.
  EXPECT_FALSE(child_status.has_value());

  Present(child, true);
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(child_status.has_value());
  ASSERT_EQ(ChildViewStatus::kContentHasPresented, *child_status);
}

TEST_F(FlatlandTest, ContentHasPresentedSignalWaitsForAcquireFences) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Client child_view_watcher(std::move(child_view_watcher_client_end), dispatcher());

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{1, 2})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  std::optional<ChildViewStatus> cvs;
  child_view_watcher->GetStatus().ThenExactlyOnce([&](ChildViewWatcher_GetStatusResult& result) {
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(ChildViewStatus::kContentHasPresented, result->status());
    cvs = result->status();
  });

  // ChildViewStatus changes as soon as the child presents. The parent does not have to present.
  EXPECT_FALSE(cvs.has_value());

  // Present the child with unsignalled acquire fence.
  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(1);
  auto acquire1_copy = utils::CopyZxHandle(args.acquire_fences[0]);
  EXPECT_FALSE(utils::IsEventSignalled(args.acquire_fences[0], ZX_EVENT_SIGNALED));
  PresentWithArgs(child, std::move(args), true);

  // Hanging get should not be run yet because the fence is not signalled.
  UpdateLinks(parent->GetRoot());
  EXPECT_FALSE(cvs.has_value());

  // Signal the acquire fence.
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  acquire1_copy.signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  ApplySessionUpdatesAndSignalFences();

  // Hanging get should run now.
  UpdateLinks(parent->GetRoot());
  EXPECT_TRUE(cvs.has_value());
  EXPECT_EQ(cvs.value(), ChildViewStatus::kContentHasPresented);
}

TEST_F(FlatlandTest, SetViewportProperties_WithDeadViewport_ShouldNotCrash) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  const TransformId kTransformId(1);
  const ContentId kLinkId(2);

  ViewportCreationToken parent_token;
  {
    ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));
    // Child token goes out of scope.
  }

  {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                          .Build();
    parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
    Present(parent, true);
  }

  {
    // Now call SetViewportProperties() and make sure we don't crash.
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena).Build();
    parent->SetViewportProperties(kLinkId, properties);
    Present(parent, true);
  }
}

TEST_F(FlatlandTest, CreateViewport_CorrectlySetsPropertiesDefaults) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId1(2);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  CreateViewport(parent.get(), child.get(), kLinkId1, std::move(child_view_watcher_server_end),
                 std::move(parent_viewport_watcher_server_end));

  {
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });
    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    ASSERT_TRUE(layout->inset().has_value());
    EXPECT_EQ(layout->logical_size()->width(), kDefaultSize);
    EXPECT_EQ(layout->logical_size()->height(), kDefaultSize);
    EXPECT_EQ(layout->inset()->top(), kDefaultInset);
    EXPECT_EQ(layout->inset()->right(), kDefaultInset);
    EXPECT_EQ(layout->inset()->bottom(), kDefaultInset);
    EXPECT_EQ(layout->inset()->left(), kDefaultInset);
  }
}

TEST_F(FlatlandTest, AfterSetViewportProperties_NewLayoutIsDeliveredWithoutPresenting) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId(2);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  {  // Create Viewport and child View.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    CreateViewport(parent.get(), child.get(), kLinkId, std::move(child_view_watcher_server_end),
                   std::move(parent_viewport_watcher_server_end));
  }

  // Get and throw away initial layout received.
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [](ParentViewportWatcher_GetLayoutResult& result) {});
  RunLoopUntilIdle();

  {  // Call SetViewportProperties(), but don't present.
    fidl::Arena arena;
    auto properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
            .logical_size(fuchsia_math::wire::SizeU{kDefaultSize + 1, kDefaultSize + 2})
            .inset(fuchsia_math::wire::Inset{
                .top = 4,
                .right = 5,
                .bottom = 6,
                .left = 7,
            })
            .Build();
    parent->SetViewportProperties(kLinkId, properties);
  }

  {  // Observe new layout being delivered immediately.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    ASSERT_TRUE(layout->inset().has_value());
    EXPECT_EQ(layout->logical_size()->width(), kDefaultSize + 1);
    EXPECT_EQ(layout->logical_size()->height(), kDefaultSize + 2);
    EXPECT_EQ(layout->inset()->top(), 4);
    EXPECT_EQ(layout->inset()->right(), 5);
    EXPECT_EQ(layout->inset()->bottom(), 6);
    EXPECT_EQ(layout->inset()->left(), 7);
  }
}

TEST_F(FlatlandTest, SetViewportProperties_BeforeLinkResolution_ShouldUpdateInitialLayout) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId1(2);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  {  // Create the Viewport.
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                          .Build();
    parent->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
    Present(parent, true);
  }

  {  // Before the child's View is created, call SetViewportProperties.
    fidl::Arena arena;
    auto properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
            .logical_size(fuchsia_math::wire::SizeU{kDefaultSize + 1, kDefaultSize + 2})
            .inset(fuchsia_math::wire::Inset{
                .top = 4,
                .right = 5,
                .bottom = 6,
                .left = 7,
            })
            .Build();
    parent->SetViewportProperties(kLinkId1, properties);
  }

  {  // Create the child View to let the link resolve.
    child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                       NoViewProtocols(), std::move(parent_viewport_watcher_server_end));
    Present(child, true);
  }

  {  // Observe that the initial layout received was updated in the SetViewProperties() call.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    ASSERT_TRUE(layout->inset().has_value());
    EXPECT_EQ(layout->logical_size()->width(), kDefaultSize + 1);
    EXPECT_EQ(layout->logical_size()->height(), kDefaultSize + 2);
    EXPECT_EQ(layout->inset()->top(), 4);
    EXPECT_EQ(layout->inset()->right(), 5);
    EXPECT_EQ(layout->inset()->bottom(), 6);
    EXPECT_EQ(layout->inset()->left(), 7);
  }
}

TEST_F(FlatlandTest, SetViewportProperties_HandlesMissingValues) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId(2);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();

    CreateViewport(parent.get(), child.get(), kLinkId, std::move(child_view_watcher_server_end),
                   std::move(parent_viewport_watcher_server_end));
  }

  {  // SetViewportProperties with only the logical size set.
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{2, 3})
                          .Build();
    parent->SetViewportProperties(kLinkId, properties);
  }

  {  // Confirm that the logical size is updated and the rest is unchanged.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    ASSERT_TRUE(layout->inset().has_value());
    EXPECT_EQ(layout->logical_size()->width(), 2u);
    EXPECT_EQ(layout->logical_size()->height(), 3u);
    EXPECT_EQ(layout->inset()->top(), kDefaultInset);
    EXPECT_EQ(layout->inset()->left(), kDefaultInset);
    EXPECT_EQ(layout->inset()->bottom(), kDefaultInset);
    EXPECT_EQ(layout->inset()->right(), kDefaultInset);
  }

  {  // Set the inset to something new.
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .inset(fuchsia_math::wire::Inset{
                              .top = 4,
                              .right = 5,
                              .bottom = 6,
                              .left = 7,
                          })
                          .Build();
    parent->SetViewportProperties(kLinkId, properties);
  }

  {  // Confirm that the insets are updated and the rest is unchanged.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    ASSERT_TRUE(layout->inset().has_value());
    EXPECT_EQ(layout->logical_size()->width(), 2u);
    EXPECT_EQ(layout->logical_size()->height(), 3u);
    EXPECT_EQ(layout->inset()->top(), 4);
    EXPECT_EQ(layout->inset()->right(), 5);
    EXPECT_EQ(layout->inset()->bottom(), 6);
    EXPECT_EQ(layout->inset()->left(), 7);
  }

  {  // Call SetViewportProperties with an empty table.
    fidl::Arena arena;
    auto empty_properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena).Build();
    parent->SetViewportProperties(kLinkId, empty_properties);
  }

  {  // Confirm that no update has been triggered.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          if (result.is_ok()) {
            layout = result->info();
          }
        });

    RunLoopUntilIdle();

    EXPECT_FALSE(layout.has_value());
  }
}

TEST_F(FlatlandTest,
       SetViewportProperties_MultipleTimes_ShouldOnlyDeliverTheNewestValue_OnHangingGet) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId(2);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    CreateViewport(parent.get(), child.get(), kLinkId, std::move(child_view_watcher_server_end),
                   std::move(parent_viewport_watcher_server_end));
  }

  const uint32_t kInitialSize = 100;

  // Set the logical size to something new multiple times.
  for (int i = 10; i >= 0; --i) {
    fidl::Arena arena;
    auto properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
            .logical_size(fuchsia_math::wire::SizeU{kInitialSize + i + 1, kInitialSize + i + 1})
            .Build();
    parent->SetViewportProperties(kLinkId, properties);
    fidl::Arena arena2;
    auto properties2 =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena2)
            .logical_size(fuchsia_math::wire::SizeU{kInitialSize + i, kInitialSize + i})
            .Build();
    parent->SetViewportProperties(kLinkId, properties2);
  }

  {  // Confirm that the callback fires, and that it has the most up-to-date data.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    EXPECT_EQ(layout->logical_size()->width(), kInitialSize);
    EXPECT_EQ(layout->logical_size()->height(), kInitialSize);
  }

  {  // Confirm that calling GetLayout again results in a hung get.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();
    ASSERT_FALSE(layout.has_value());

    // Update the properties again and observe the pending GetLayout() triggering.
    const uint32_t kNewSize = 50u;
    {
      fidl::Arena arena;
      auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                            .logical_size(fuchsia_math::wire::SizeU{kNewSize, kNewSize})
                            .Build();
      parent->SetViewportProperties(kLinkId, properties);
    }
    RunLoopUntilIdle();

    // Confirm that we receive the update and that it had the newest values.
    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    EXPECT_EQ(layout->logical_size()->width(), kNewSize);
    EXPECT_EQ(layout->logical_size()->height(), kNewSize);
  }
}

TEST_F(FlatlandTest, SetViewportProperties_OnMultipleChildren_ShouldUpdateEachOne) {
  const int kNumChildren = 3;
  const ContentId kLinkIds[kNumChildren] = {ContentId(5), ContentId(6), ContentId(7)};

  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> children[kNumChildren] = {CreateFlatland(), CreateFlatland(),
                                                      CreateFlatland()};

  fidl::Client<ParentViewportWatcher> parent_viewport_watcher[kNumChildren];

  for (int i = 0; i < kNumChildren; ++i) {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::Endpoints<ParentViewportWatcher>::Create();
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();

    CreateViewport(parent.get(), children[i].get(), kLinkIds[i],
                   std::move(child_view_watcher_server_end),
                   std::move(parent_viewport_watcher_server_end));

    parent_viewport_watcher[i].Bind(std::move(parent_viewport_watcher_client_end), dispatcher());
  }

  // Confirm that all children are at the default value
  for (int i = 0; i < kNumChildren; ++i) {
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher[i]->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          ASSERT_TRUE(result->info().logical_size().has_value());
          EXPECT_EQ(kDefaultSize, result->info().logical_size()->width());
          EXPECT_EQ(kDefaultSize, result->info().logical_size()->height());
          layout = result->info();
        });

    RunLoopUntilIdle();
  }

  // Resize the content on all children.
  for (auto id : kLinkIds) {
    fidl::Arena arena;
    auto properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
            .logical_size(fuchsia_math::wire::SizeU{static_cast<uint32_t>(id.value()),
                                                    static_cast<uint32_t>(id.value() * 2)})
            .Build();
    parent->SetViewportProperties(id, properties);
  }

  // Confirm that all children are updated.
  for (int i = 0; i < kNumChildren; ++i) {
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher[i]->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->logical_size().has_value());
    EXPECT_EQ(kLinkIds[i].value(), layout->logical_size()->width());
    EXPECT_EQ(kLinkIds[i].value() * 2, layout->logical_size()->height());
  }
}

TEST_F(FlatlandTest, LinkSystem_WhenSettingDevicePixelRatio_ItShouldBeTransmittedToLayoutInfo) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  const ContentId kLinkId(2);

  const glm::vec2 initial_dpr = {3.f, 4.f};
  link_system_->UpdateDevicePixelRatio(initial_dpr);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  {
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    fidl::Client child_view_watcher(std::move(child_view_watcher_client_end), dispatcher());

    CreateViewport(parent.get(), child.get(), kLinkId, std::move(child_view_watcher_server_end),
                   std::move(parent_viewport_watcher_server_end));
  }

  {  // Observe the DPR in the initial received layout.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });

    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->device_pixel_ratio().has_value());
    EXPECT_EQ(layout->device_pixel_ratio()->x(), initial_dpr.x);
    EXPECT_EQ(layout->device_pixel_ratio()->y(), initial_dpr.y);
  }

  // Set a new DPR.
  const glm::vec2 new_dpr = {5.f, 6.f};
  link_system_->UpdateDevicePixelRatio(new_dpr);

  {  // Observe the new DPR being delivered.
    std::optional<LayoutInfo> layout;
    parent_viewport_watcher->GetLayout().ThenExactlyOnce(
        [&](ParentViewportWatcher_GetLayoutResult& result) {
          ASSERT_TRUE(result.is_ok());
          layout = result->info();
        });
    RunLoopUntilIdle();

    ASSERT_TRUE(layout.has_value());
    ASSERT_TRUE(layout->device_pixel_ratio().has_value());
    EXPECT_EQ(layout->device_pixel_ratio()->x(), new_dpr.x);
    EXPECT_EQ(layout->device_pixel_ratio()->y(), new_dpr.y);
  }
}

TEST_F(FlatlandTest, SetLinkOnTransformErrorCases) {
  const TransformId kId1(1);
  const TransformId kId2(2);

  const ContentId kLinkId1(1);
  const ContentId kLinkId2(2);

  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Creating a link with an empty property object is an error. Logical size must be provided at
    // creation time.
    ViewportCreationToken parent_token;
    ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));
    fidl::Arena arena;
    auto empty_properties =
        fuchsia_ui_composition::wire::ViewportProperties::Builder(arena).Build();

    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), empty_properties,
                             std::move(child_view_watcher_server_end));

    Present(flatland, false);
  }

  // Setup.
  auto SetupFlatland = [&]() {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    ViewportCreationToken parent_token;
    ViewCreationToken child_token;
    zx::channel::create(0, &parent_token.value(), &child_token.value());
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                          .Build();

    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                             std::move(child_view_watcher_server_end));
    return flatland;
  };

  // Zero is not a valid transform_id.
  {
    auto flatland = SetupFlatland();
    Present(flatland, true);

    flatland->SetContent(kInvalidTransformId, kLinkId1);
    Present(flatland, false);
  }

  // Setting a valid link on an invalid transform is not valid.
  {
    auto flatland = SetupFlatland();
    Present(flatland, true);

    flatland->SetContent(kId2, kLinkId1);
    Present(flatland, false);
  }

  // Setting an invalid link on a valid transform is not valid.
  {
    auto flatland = SetupFlatland();
    flatland->CreateTransform(kId1);
    Present(flatland, true);

    flatland->SetContent(kId1, kLinkId2);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, ReleaseViewportErrorCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();

  // Zero is not a valid link_id.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->ReleaseViewport(
        kInvalidContentId,
        [](fuchsia_ui_views::wire::ViewportCreationToken token) { EXPECT_TRUE(false); });
    Present(flatland, false);
  }

  // Using a link_id that does not exist is not valid.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    const ContentId kLinkId1(1);
    flatland->ReleaseViewport(
        kLinkId1, [](fuchsia_ui_views::wire::ViewportCreationToken token) { EXPECT_TRUE(false); });
    Present(flatland, false);
  }

  // ContentId is not a Link.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    const ContentId kImageId(2);
    auto ref_pair = BufferCollectionImportExportTokens::New();

    ImageProperties properties;
    properties.size(SizeU{100, 200});

    CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
                std::move(properties));

    flatland->ReleaseViewport(
        kImageId, [](fuchsia_ui_views::wire::ViewportCreationToken token) { EXPECT_TRUE(false); });
    Present(flatland, false);
  }
}

// In most of the tests, we invoke methods directly on a shared_ptr<Flatland> rather than over a
// FIDL channel.  In most cases, this exercises the code sufficiently.  However, `ReleaseViewport()`
// is different because has a return value, which means that its `fidl::Completer` will close the
// channel if it is not completed.
TEST_F(FlatlandTest, ReleaseViewportViaFidlClient) {
  const ContentId kViewportId(1);

  // Create a viewport.  Returns the ChildViewWatcher client-end, to allow it to be kept alive.
  auto CreateViewport =
      [&](FlatlandEventLoopClientServer& client_server,
          ContentId viewport_id) -> fidl::ClientEnd<fuchsia_ui_composition::ChildViewWatcher> {
    ViewportCreationToken parent_token;
    ViewCreationToken child_token;
    EXPECT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));
    auto child_view_watcher_endpoints = fidl::Endpoints<ChildViewWatcher>::Create();

    libsync::Completion completion;
    async::PostTask(client_server.server_loop().dispatcher(), [&]() {
      fidl::Arena arena;
      auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                            .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                            .Build();
      client_server.server()->CreateViewport(viewport_id, ToWire(std::move(parent_token)),
                                             properties,
                                             std::move(child_view_watcher_endpoints.server));
      Present(client_server.server(), true);
      completion.Signal();
    });
    completion.Wait();

    return std::move(child_view_watcher_endpoints.client);
  };

  {
    auto client_server = CreateFlatlandEventLoopClientServer();
    auto child_view_watcher_client_end = CreateViewport(*client_server, kViewportId);

    // Illegally invoke `ReleaseViewport()` and wait for the response, which requires a server
    // round-trip.
    libsync::Completion completion;
    async::PostTask(client_server->client_loop().dispatcher(), [&]() {
      (*client_server->client())
          ->ReleaseViewport(kInvalidContentId.ToFidl())
          .ThenExactlyOnce(
              [&](fidl::Result<fuchsia_ui_composition::Flatland::ReleaseViewport>& result) {
                ASSERT_TRUE(result.is_error());
                completion.Signal();
              });
    });
    completion.Wait();
  }

  {
    auto client_server = CreateFlatlandEventLoopClientServer();
    auto child_view_watcher_client_end = CreateViewport(*client_server, kViewportId);

    // Illegally invoke `ReleaseViewport()` and wait for the response, which requires a server
    // round-trip.
    libsync::Completion completion;
    async::PostTask(client_server->client_loop().dispatcher(), [&]() {
      const ContentId kNonExistentViewportId(420);
      (*client_server->client())
          ->ReleaseViewport(kNonExistentViewportId.ToFidl())
          .ThenExactlyOnce(
              [&](fidl::Result<fuchsia_ui_composition::Flatland::ReleaseViewport>& result) {
                ASSERT_TRUE(result.is_error());
                completion.Signal();
              });
    });
    completion.Wait();
  }

  {
    auto client_server = CreateFlatlandEventLoopClientServer();
    auto child_view_watcher_client_end = CreateViewport(*client_server, kViewportId);

    // Release the viewport.  This will not complete on the server.
    libsync::Completion completion;
    async::PostTask(client_server->client_loop().dispatcher(), [&]() {
      (*client_server->client())
          ->ReleaseViewport(kViewportId.ToFidl())
          .ThenExactlyOnce(
              [&](fidl::Result<fuchsia_ui_composition::Flatland::ReleaseViewport>& result) {
                ASSERT_TRUE(result.is_ok());
                completion.Signal();
              });
    });
    // We don't want to drastically increase the time takes to run, so wait only 100ms.
    // We "know" this would timeout no matter how long we wait, but
    zx_status_t result = completion.Wait(zx::duration(100'000'000));
    EXPECT_EQ(result, ZX_ERR_TIMED_OUT);

    // Now present.  We do this via the FIDL client to ensure correct sequencing (i.e. this way the
    // `Present()` is guaranteed to follow the `ReleaseViewport()`).
    EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
    async::PostTask(client_server->client_loop().dispatcher(), [&]() {
      fuchsia_ui_composition::PresentArgs present_args;
      present_args.requested_presentation_time(0)
          .acquire_fences({})
          .release_fences({})
          .unsquashable(true);
      auto result = (*client_server->client())->Present(std::move(present_args));
      ASSERT_TRUE(result.is_ok());
    });

    completion.Wait();
  }
}

TEST_F(FlatlandTest, ReleaseViewportReturnsOriginalToken) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const zx_koid_t expected_koid = fsl::GetKoid(parent_token.value().get());

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, true);

  fuchsia_ui_views::wire::ViewportCreationToken content_token;
  flatland->ReleaseViewport(kLinkId1,
                            [&content_token](fuchsia_ui_views::wire::ViewportCreationToken token) {
                              content_token = std::move(token);
                            });

  RunLoopUntilIdle();

  // Until Present() is called and the acquire fence is signaled, the previous ChildViewWatcher is
  // not unbound.
  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_FALSE(content_token.value.is_valid());

  PresentArgs args;
  args.acquire_fences = utils::CreateEventArray(1);
  auto event_copy = utils::CopyZxHandle(args.acquire_fences[0]);

  PresentWithArgs(flatland, std::move(args), true);

  EXPECT_TRUE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_FALSE(content_token.value.is_valid());

  // Signal the acquire fence to unbind the link.
  event_copy.signal(0, ZX_EVENT_SIGNALED);

  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  RunLoopUntilIdle();

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end));
  EXPECT_TRUE(content_token.value.is_valid());
  EXPECT_EQ(fsl::GetKoid(content_token.value.get()), expected_koid);
}

TEST_F(FlatlandTest, ReleaseViewportReturnsOrphanedTokenOnChildDeath) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  flatland->CreateViewport(kLinkId1, ToWire(std::move(parent_token)), properties,
                           std::move(child_view_watcher_server_end));
  Present(flatland, true);

  // Killing the peer token does not prevent the instance from returning a valid token.
  child_token.value().reset();
  RunLoopUntilIdle();

  fuchsia_ui_views::wire::ViewportCreationToken content_token;
  flatland->ReleaseViewport(kLinkId1,
                            [&content_token](fuchsia_ui_views::wire::ViewportCreationToken token) {
                              content_token = std::move(token);
                            });
  Present(flatland, true);

  EXPECT_TRUE(content_token.value.is_valid());

  // But trying to link with that token will immediately fail because it is already orphaned.
  const ContentId kLinkId2(2);

  auto [child_view_watcher_client_end2, child_view_watcher_server_end2] =
      fidl::Endpoints<ChildViewWatcher>::Create();

  flatland->CreateViewport(kLinkId2, std::move(content_token), properties,
                           std::move(child_view_watcher_server_end2));
  Present(flatland, true);

  EXPECT_FALSE(ClientEndPeerExists(child_view_watcher_client_end2));
}

TEST_F(FlatlandTest, CreateViewportPresentedBeforeCreateView) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Create a transform, add it to the parent, then create a link and assign to the transform.
  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kId1, kLinkId);

  Present(parent, true);

  // Link the child to the parent->
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  // The child should only be accessible from the parent when Present() is called on the child->
  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  Present(child, true);

  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));
}

TEST_F(FlatlandTest, CreateViewPresentedBeforeCreateViewport) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Link the child to the parent
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();

  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  Present(child, true);

  // Create a transform, add it to the parent, then create a link and assign to the transform.
  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);

  // Present the parent once so that it has a topology or else IsDescendantOf() will crash.
  Present(parent, true);

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kId1, kLinkId);

  // The child should only be accessible from the parent when Present() is called on the parent->
  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  Present(parent, true);

  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));
}

TEST_F(FlatlandTest, LinkResolvedBeforeEitherPresent) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Create a transform, add it to the parent, then create a link and assign to the transform.
  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);

  // Present the parent once so that it has a topology or else IsDescendantOf() will crash.
  Present(parent, true);

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kId1, kLinkId);

  // Link the child to the parent->
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  // The child should only be accessible from the parent when Present() is called on both the
  // parent and the child->
  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  Present(parent, true);

  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  Present(child, true);

  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));
}

TEST_F(FlatlandTest, ClearLinkToChild) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  ViewportCreationToken parent_token;
  ViewCreationToken child_token;
  ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

  // Create and link the two instances.
  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);

  const ContentId kLinkId(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                         std::move(child_view_watcher_server_end));
  parent->SetContent(kId1, kLinkId);

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  child->CreateView2(ToWire(std::move(child_token)), NewWireViewIdentityOnCreation(),
                     NoViewProtocols(), std::move(parent_viewport_watcher_server_end));

  Present(parent, true);
  Present(child, true);

  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  // Reset the child link using zero as the link id.
  parent->SetContent(kId1, kInvalidContentId);

  Present(parent, true);

  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));
}

TEST_F(FlatlandTest, RecreateReleasedLinkSameToken) {
  std::shared_ptr<Flatland> parent = CreateFlatland();
  std::shared_ptr<Flatland> child = CreateFlatland();

  // Create link and Present.
  const ContentId kLinkId1(1);

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  zx::unowned_channel unowned_child_view_watcher_client_end(
      child_view_watcher_client_end.channel().get());
  fidl::Client child_view_watcher(std::move(child_view_watcher_client_end), dispatcher());

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  zx::unowned_channel unowned_parent_viewport_watcher_client_end(
      parent_viewport_watcher_client_end.channel().get());
  fidl::Client parent_viewport_watcher(std::move(parent_viewport_watcher_client_end), dispatcher());

  CreateViewport(parent.get(), child.get(), kLinkId1, std::move(child_view_watcher_server_end),
                 std::move(parent_viewport_watcher_server_end));
  RunLoopUntilIdle();

  const TransformId kId1(1);
  parent->CreateTransform(kId1);
  parent->SetRootTransform(kId1);
  parent->SetContent(kId1, kLinkId1);
  Present(parent, true);
  EXPECT_TRUE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  // Both protocols should be bound at this point.
  EXPECT_TRUE(PeerExists(unowned_child_view_watcher_client_end));
  bool child_view_watcher_updated = false;
  child_view_watcher->GetStatus().ThenExactlyOnce(
      [&child_view_watcher_updated](ChildViewWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        child_view_watcher_updated = true;
      });
  EXPECT_FALSE(child_view_watcher_updated);
  UpdateLinks(parent->GetRoot());
  RunLoopUntilIdle();
  EXPECT_TRUE(child_view_watcher_updated);

  EXPECT_TRUE(PeerExists(unowned_parent_viewport_watcher_client_end));

  // Release the link on parent.
  fuchsia_ui_views::wire::ViewportCreationToken content_token;
  parent->ReleaseViewport(kLinkId1,
                          [&content_token](fuchsia_ui_views::wire::ViewportCreationToken token) {
                            content_token = std::move(token);
                          });
  Present(parent, true);
  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  // The same token can be used to create a different link to the same child with a different
  // parent.
  std::shared_ptr<Flatland> parent2 = CreateFlatland();
  const TransformId kId2(2);
  parent2->CreateTransform(kId2);
  parent2->SetRootTransform(kId2);

  auto [child_view_watcher_client_end2, child_view_watcher_server_end2] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  zx::unowned_channel unowned_child_view_watcher_client_end2(
      child_view_watcher_client_end2.channel().get());
  fidl::Client child_view_watcher2(std::move(child_view_watcher_client_end2), dispatcher());

  const ContentId kLinkId2(2);
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent2->CreateViewport(kLinkId2, std::move(content_token), properties,
                          std::move(child_view_watcher_server_end2));
  parent2->SetContent(kId2, kLinkId2);
  Present(parent2, true);
  EXPECT_TRUE(IsDescendantOf(parent2->GetRoot(), child->GetRoot()));

  // The old instance is not re-linked.
  EXPECT_FALSE(IsDescendantOf(parent->GetRoot(), child->GetRoot()));

  // Both protocols should still be bound.
  EXPECT_TRUE(PeerExists(unowned_child_view_watcher_client_end2));
  bool child_view_watcher_updated2 = false;
  child_view_watcher2->GetStatus().ThenExactlyOnce(
      [&child_view_watcher_updated2](ChildViewWatcher_GetStatusResult& result) {
        ASSERT_TRUE(result.is_ok());
        child_view_watcher_updated2 = true;
      });
  EXPECT_FALSE(child_view_watcher_updated2);
  UpdateLinks(parent->GetRoot());
  RunLoopUntilIdle();
  EXPECT_TRUE(child_view_watcher_updated2);

  EXPECT_TRUE(PeerExists(unowned_parent_viewport_watcher_client_end));
  bool parent_viewport_watcher_updated = false;
  parent_viewport_watcher->GetLayout().ThenExactlyOnce(
      [&](ParentViewportWatcher_GetLayoutResult& result) {
        ASSERT_TRUE(result.is_ok());
        parent_viewport_watcher_updated = true;
      });
  EXPECT_FALSE(parent_viewport_watcher_updated);
  RunLoopUntilIdle();
  EXPECT_TRUE(parent_viewport_watcher_updated);
}

TEST_F(FlatlandTest, CreateImageValidCase) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(SizeU{100, 200});

  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
}

TEST_F(FlatlandTest, SetImageOpacityTestCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  const TransformId kTransformId(3);
  const ContentId kId(1);
  const ContentId kIdChild(2);

  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageOpacity(kInvalidContentId, 0.5);
    Present(flatland, false);
  }

  // The content id hasn't been imported yet.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageOpacity(kId, 0.5);
    Present(flatland, false);
  }

  // Trying to set opacity on a solid color.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kId);
    flatland->SetImageOpacity(kId, 0.5);
    Present(flatland, false);
  }

  // The alpha values are out of range.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid image
    auto ref_pair_1 = BufferCollectionImportExportTokens::New();

    ImageProperties properties1;
    properties1.size(SizeU{100, 200});

    auto import_token_dup = ref_pair_1.DuplicateImportToken();
    const allocation::GlobalBufferCollectionId global_collection_id1 =
        CreateImage(flatland.get(), allocator.get(), kId, std::move(ref_pair_1),
                    std::move(properties1))
            .collection_id;

    flatland->CreateTransform(kTransformId);
    flatland->SetRootTransform(kTransformId);
    flatland->SetContent(kTransformId, kId);
    flatland->SetImageOpacity(kId, -0.5);
    Present(flatland, false);
  }
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid image
    auto ref_pair_1 = BufferCollectionImportExportTokens::New();

    ImageProperties properties1;
    properties1.size(SizeU{100, 200});

    auto import_token_dup = ref_pair_1.DuplicateImportToken();
    const allocation::GlobalBufferCollectionId global_collection_id1 =
        CreateImage(flatland.get(), allocator.get(), kId, std::move(ref_pair_1),
                    std::move(properties1))
            .collection_id;

    flatland->CreateTransform(kTransformId);
    flatland->SetRootTransform(kTransformId);
    flatland->SetContent(kTransformId, kId);
    flatland->SetImageOpacity(kId, 1.5);
    Present(flatland, false);
  }

  // Testing now with good values should finally work.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid image
    auto ref_pair_1 = BufferCollectionImportExportTokens::New();

    ImageProperties properties1;
    properties1.size(SizeU{100, 200});

    auto import_token_dup = ref_pair_1.DuplicateImportToken();
    const allocation::GlobalBufferCollectionId global_collection_id1 =
        CreateImage(flatland.get(), allocator.get(), kId, std::move(ref_pair_1),
                    std::move(properties1))
            .collection_id;

    flatland->CreateTransform(kTransformId);
    flatland->SetRootTransform(kTransformId);
    flatland->SetContent(kTransformId, kId);
    flatland->SetImageOpacity(kId, 0.7f);
    Present(flatland, true);
  }
}

TEST_F(FlatlandTest, SetTransformOpacityTestCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  const TransformId kId(1);
  const TransformId kIdChild(2);

  // Zero is not a valid transform ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetOpacity(kInvalidTransformId, 0.5);
    Present(flatland, false);
  }

  // The transform id hasn't been imported yet.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetOpacity(kId, 0.5);
    Present(flatland, false);
  }

  // The alpha values are out of range.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid transform.
    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetOpacity(kId, -0.5);
    Present(flatland, false);
  }
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid transform.
    flatland->CreateTransform(kId);
    flatland->SetRootTransform(kId);
    flatland->SetOpacity(kId, 1.5);
    Present(flatland, false);
  }

  // Testing now with good values should finally work.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    // Setup a valid transform.
    flatland->CreateTransform(kId);
    Present(flatland, true);
    flatland->SetRootTransform(kId);
    flatland->SetOpacity(kId, 0.5);
    Present(flatland, true);
  }
}

TEST_F(FlatlandTest, CreateFilledRectErrorTest) {
  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kInvalidContentId);
    Present(flatland, false);
  }

  // Same ID can't be imported twice.
  {
    const ContentId kId(1);
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kId);
    Present(flatland, true);

    flatland->CreateFilledRect(kId);
    Present(flatland, false);
  }

  // Test SetSolidFill function.
  {
    const ContentId kId2(2);

    // Can't call SetSolidFill on invalid ID.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->SetSolidFill(kInvalidContentId, {1, 0, 0, 1}, {20, 30});
      Present(flatland, false);
    }

    // Can't call SetSolidFill on ID that hasn't been created.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->SetSolidFill(kId2, {1, 0, 0, 1}, {20, 30});
      Present(flatland, false);
    }

    // Now it should work after creating the filled rect first.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->CreateFilledRect(kId2);
      flatland->SetSolidFill(kId2, {1, 0, 0, 1}, {20, 30});
      Present(flatland, true);
    }

    // Try various values and make sure present still returns true.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->CreateFilledRect(kId2);
      flatland->SetSolidFill(kId2, {0.7f, 0.3f, 0.9f, 0.4f}, {20, 30});
      Present(flatland, true);
    }
  }

  // Test ReleaseFilledRect function
  {
    const ContentId kId3(3);

    // Cannot release an invalid ID.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->ReleaseFilledRect(kInvalidContentId);
      Present(flatland, false);
    }

    // Cannot release an ID that hasn't been created.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->ReleaseFilledRect(kId3);
      Present(flatland, false);
    }

    // Now it should work once we create it first.
    {
      std::shared_ptr<Flatland> flatland = CreateFlatland();
      flatland->CreateFilledRect(kId3);
      flatland->ReleaseFilledRect(kId3);
      Present(flatland, true);

      // And now we should be able to reuse the same id.
      flatland->CreateFilledRect(kId3);
      Present(flatland, true);
    }
  }
}

TEST_F(FlatlandTest, SetImageSampleRegionTestCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  const TransformId kTransformId(1);
  const ContentId kId(3);
  const uint32_t kImageWidth = 854;
  const uint32_t kImageHeight = 480;

  // Zero is not a valid content ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageSampleRegion(kInvalidContentId,
                                   types::RectangleF({0, 0, kImageWidth, kImageHeight}));
    Present(flatland, false);
  }

  // The content id hasn't been imported yet.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetImageSampleRegion(kId, types::RectangleF({0, 0, kImageWidth, kImageHeight}));
    Present(flatland, false);
  }

  // Setup a valid transform and image.
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);

  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(fuchsia_math::SizeU{kImageWidth, kImageHeight});

  CreateImage(flatland.get(), allocator.get(), kId, std::move(ref_pair), std::move(properties));

  flatland->SetContent(kTransformId, kId);
  Present(flatland, true);

  // Testing now with good values should finally work.
  {
    flatland->SetImageSampleRegion(kId, types::RectangleF({0, 0, kImageWidth, kImageHeight}));
    Present(flatland, true);

    const float kYDeltaCoefficient = 0.370833f;
    const float kHeightCoefficient = 0.629167f;
    EXPECT_EQ(1.f, kYDeltaCoefficient + kHeightCoefficient);
    const float kYDelta = kYDeltaCoefficient * kImageHeight;
    const float kYHeight = kHeightCoefficient * kImageHeight;
    // Note that comparing these values using == operator as floats fails due to the loss of
    // precision.
    EXPECT_GT(kYDelta + kYHeight, static_cast<float>(kImageHeight));
    EXPECT_TRUE(escher::CompareFloat(kYDelta + kYHeight, static_cast<float>(kImageHeight), 0.01f));
    flatland->SetImageSampleRegion(kId, types::RectangleF({0, kYDelta, kImageWidth, kYHeight}));
    Present(flatland, true);
  }

  // Test one more time with out of bounds values and it should fail.
  {
    // (x + width) exceeeds the image width and should fail.
    flatland->SetImageSampleRegion(kId, types::RectangleF({1, 0, kImageWidth, kImageHeight}));
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetClipBoundaryErrorCases) {
  const TransformId kTransformId(1);

  // Zero is not a valid transform ID.
  {
    fuchsia_math::wire::Rect rect{0, 0, 20, 30};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetClipBoundary(kInvalidTransformId, rect);
    Present(flatland, false);
  }

  // Transform ID is valid but not yet imported
  {
    fuchsia_math::wire::Rect rect{0, 0, 20, 30};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, false);
  }

  // Width must be positive.
  {
    fuchsia_math::wire::Rect rect{0, 0, 20, 30};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kTransformId);
    flatland->SetRootTransform(kTransformId);
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, true);

    const auto maybe_transform_handle = flatland->GetTransformHandle(kTransformId);
    ASSERT_TRUE(maybe_transform_handle.has_value());
    const auto transform_handle = maybe_transform_handle.value();

    auto uber_struct = GetUberStruct(flatland.get());
    auto clip_region_itr = uber_struct->local_clip_regions.find(transform_handle);
    EXPECT_NE(clip_region_itr, uber_struct->local_clip_regions.end());
    auto clip_region = clip_region_itr->second;
    EXPECT_EQ(TransformClipRegion::From(rect), clip_region);

    fuchsia_math::wire::Rect rect_bad = {0, 0, -20, 30};
    flatland->SetClipBoundary(kTransformId, rect_bad);
    Present(flatland, false);
  }

  // Height must be positive.
  {
    fuchsia_math::wire::Rect rect{0, 0, 20, 30};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kTransformId);
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, true);

    fuchsia_math::wire::Rect rect_bad = {0, 0, 20, -30};
    flatland->SetClipBoundary(kTransformId, rect_bad);
    Present(flatland, false);
  }

  // Can't overflow on the X-axis.
  {
    fuchsia_math::wire::Rect rect = {INT_MAX - 1, 0, INT_MAX - 1, 30};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kTransformId);
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, false);
  }

  // Can't overflow on the Y-axis.
  {
    fuchsia_math::wire::Rect rect = {0, INT_MAX - 1, 30, INT_MAX - 1};
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kTransformId);
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, false);
  }

  // Null value is OK.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateTransform(kTransformId);
    flatland->SetRootTransform(kTransformId);

    // Grab the transform handle.
    const auto maybe_transform_handle = flatland->GetTransformHandle(kTransformId);
    ASSERT_TRUE(maybe_transform_handle.has_value());
    const auto transform_handle = maybe_transform_handle.value();

    // Set a null value.
    flatland->SetClipBoundary(kTransformId, std::nullopt);
    Present(flatland, true);

    // Check that there is no clip region in the uber struct.
    auto uber_struct = GetUberStruct(flatland.get());
    EXPECT_FALSE(uber_struct->local_clip_regions.contains(transform_handle));

    // Set a proper value.
    fuchsia_math::wire::Rect rect = {10, 30, 20, 90};
    flatland->SetClipBoundary(kTransformId, rect);
    Present(flatland, true);

    // Check that this value has now made its way to the uber struct.
    uber_struct = GetUberStruct(flatland.get());
    auto clip_region_itr = uber_struct->local_clip_regions.find(transform_handle);
    ASSERT_NE(clip_region_itr, uber_struct->local_clip_regions.end());
    auto clip_region = clip_region_itr->second;
    EXPECT_EQ(TransformClipRegion::From(rect), clip_region);

    // Set it to be null again.
    flatland->SetClipBoundary(kTransformId, std::nullopt);
    Present(flatland, true);

    // Now check that its not in the uber struct anymore.
    uber_struct = GetUberStruct(flatland.get());
    EXPECT_FALSE(uber_struct->local_clip_regions.contains(transform_handle));
  }
}

TEST_F(FlatlandTest, CreateImageErrorCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();

  // Default image properties.
  const uint32_t kDefaultVmoIndex = 1;
  const uint32_t kDefaultWidth = 100;
  const uint32_t kDefaultHeight = 1000;

  // Setup a valid buffer collection.
  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  // Zero is not a valid image ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateImage(kInvalidContentId, ToWire(ref_pair.DuplicateImportToken()),
                          kDefaultVmoIndex, {});
    Present(flatland, false);
  }

  // The import token must also be valid.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateImage(ContentId(1), fuchsia_ui_composition::wire::BufferCollectionImportToken{},
                          kDefaultVmoIndex, {});
    Present(flatland, false);
  }

  // The buffer collection can fail to create an image.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          {});
    Present(flatland, false);
  }

  // Size must be set.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          {});
    Present(flatland, false);
  }

  // Width cannot be 0.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{0, 1})
                          .Build();
    flatland->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          properties);
    Present(flatland, false);
  }

  // Height cannot be 0.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{1, 0})
                          .Build();
    flatland->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          properties);
    Present(flatland, false);
  }

  // Check to make sure that if the BufferCollectionImporter returns false, then the call
  // to Flatland::CreateImage() also returns false.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    const ContentId kId(100);
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{kDefaultWidth, kDefaultHeight})
                          .Build();
    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::error()));
    flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          properties);
    RunLoopUntilIdle();
    Present(flatland, false);
  }

  // Two images cannot have the same ID.
  const ContentId kId(1);
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    {
      fidl::Arena arena;
      auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                            .size(fuchsia_math::wire::SizeU{kDefaultWidth, kDefaultHeight})
                            .Build();

      // This is the first call in these series of test components that makes it down to
      // the BufferCollectionImporter. We have to make sure it returns true here so that
      // the test doesn't erroneously fail.
      EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
          .WillOnce(ReturnPromise(fpromise::ok()));

      flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                            properties);
      Present(flatland, true);
    }

    {
      fidl::Arena arena;
      auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                            .size(fuchsia_math::wire::SizeU{kDefaultWidth, kDefaultHeight})
                            .Build();

      // We shouldn't even make it to the BufferCollectionImporter here due to the duplicate
      // ID causing CreateImage() to return early.
      EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _)).Times(0);
      flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                            properties);
      Present(flatland, false);
    }
  }

  // A Link id cannot be used for an image.
  const ContentId kLinkId(2);
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    ViewportCreationToken parent_token;
    ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    fidl::Arena arena;
    auto link_properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                               .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                               .Build();
    flatland->CreateViewport(kLinkId, ToWire(std::move(parent_token)), link_properties,
                             std::move(child_view_watcher_server_end));
    Present(flatland, true);

    fidl::Arena arena2;
    auto image_properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena2)
                                .size(fuchsia_math::wire::SizeU{kDefaultWidth, kDefaultHeight})
                                .Build();

    flatland->CreateImage(kLinkId, ToWire(ref_pair.DuplicateImportToken()), kDefaultVmoIndex,
                          image_properties);
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, CreateImageWithDuplicatedImportTokens) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const uint64_t kNumImages = 3;
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .Times(kNumImages)
      .WillRepeatedly(ReturnPromise(fpromise::ok()));

  for (uint64_t i = 0; i < kNumImages; ++i) {
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{150, 175})
                          .Build();
    flatland->CreateImage(/*image_id*/ ContentId(i + 1), ToWire(ref_pair.DuplicateImportToken()),
                          /*vmo_idx*/ static_cast<uint32_t>(i), properties);
    Present(flatland, true);
  }
}

TEST_F(FlatlandTest, CreateImageAsyncWaitsForPresent) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fpromise::bridge<> bridge;

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(testing::Invoke(
          [&bridge](const ImageMetadata& metadata, allocation::BufferCollectionUsage usage_type) {
            return bridge.consumer.promise();
          }));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();

  const ContentId kId(1);
  flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);

  // Call Present.
  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  // Run loop. The task should be blocked on the fence.
  RunLoopUntilIdle();

  // Now resolve the promise.
  bridge.completer.complete_ok();

  // Run loop again. Now the fence should be signaled and the task should run.
  // Note: if this `EXPECT_CALL` wasn't here, the test would fail due to an
  // unexpected call on the presenter.
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));

  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, DestroyFlatlandBeforeCreateImageResolves) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fpromise::bridge<> bridge;

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(testing::Invoke(
          [&bridge](const ImageMetadata& metadata, allocation::BufferCollectionUsage usage_type) {
            return bridge.consumer.promise();
          }));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();

  const ContentId kId(1);
  flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);

  // Run loop to get the task scheduled on the executor.
  RunLoopUntilIdle();

  // Now destroy the flatland instance.
  flatland.reset();

  // Run loop again. The pending task should be dropped because the executor is
  // destroyed, i.e. the mock receives no call to ScheduleUpdateForSession().
  // There should be no crash.
  RunLoopUntilIdle();

  // We can also resolve the promise to ensure it doesn't cause issues.
  bridge.completer.complete_ok();
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, ReleaseImageBeforeAsyncCreateImageCompletes) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fpromise::bridge<> bridge;
  allocation::GlobalImageId global_image_id = allocation::kInvalidImageId;

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(testing::Invoke(
          [&bridge, &global_image_id](const ImageMetadata& metadata,
                                      allocation::BufferCollectionUsage usage_type) {
            global_image_id = metadata.identifier;
            return bridge.consumer.promise();
          }));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();

  const ContentId kId(1);
  flatland->CreateImage(kId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);

  // Release the image before the promise is resolved.
  flatland->ReleaseImage(kId);

  // Call Present.
  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  // Run loop to get the task scheduled on the executor. The task should be blocked on the fence.
  RunLoopUntilIdle();

  // Now resolve the promise.
  bridge.completer.complete_ok();

  // The session update should run and schedule an update, but the image is already marked dead.
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));

  RunLoopUntilIdle();

  // Once the session update is applied and presentation completed (simulated by signaling the
  // release fences), the image must be released by the importer.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);

  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, CreateImageInMultipleFlatlands) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland1 = CreateFlatland();
  std::shared_ptr<Flatland> flatland2 = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  // We can import the same image in both flatland instances.
  {
    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{150, 175})
                          .Build();
    flatland1->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
    Present(flatland1, true);
  }
  {
    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                          .size(fuchsia_math::wire::SizeU{150, 175})
                          .Build();
    flatland2->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
    Present(flatland2, true);
  }

  // There are seperate ReleaseBufferImage calls to release them from importers.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(2);
  flatland1->Clear();
  Present(flatland1, true);
  flatland2->Clear();
  Present(flatland2, true);
}

TEST_F(FlatlandTest, SetContentErrorCases) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  const uint32_t kWidth = 100;
  const uint32_t kHeight = 200;

  ImageProperties properties;
  properties.size(SizeU{kWidth, kHeight});

  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));

  // Create a transform.
  const TransformId kTransformId(1);

  flatland->CreateTransform(kTransformId);
  Present(flatland, true);

  // Zero is not a valid transform.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetContent(kInvalidTransformId, kImageId);
    Present(flatland, false);
  }

  // The transform must exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetContent(TransformId(2), kImageId);
    Present(flatland, false);
  }

  // The image must exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->SetContent(kTransformId, ContentId(2));
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, ClearContentOnTransform) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties;
  properties.size(SizeU{100, 200});

  auto import_token_dup = ref_pair.DuplicateImportToken();
  auto global_collection_id = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties))
                                  .collection_id;

  const auto maybe_image_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_image_handle.has_value());
  const auto image_handle = maybe_image_handle.value();

  // Create a transform, make it the root transform, and attach the image.
  const TransformId kTransformId(1);

  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);

  // The image handle should be the last handle in the local_topology, and the image should be in
  // the image map.
  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_EQ(uber_struct->local_topology.back().handle, image_handle);
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle));

  // An ContentId of 0 indicates to remove any content on the specified transform.
  flatland->SetContent(kTransformId, kInvalidContentId);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  for (const auto& entry : uber_struct->local_topology) {
    EXPECT_NE(entry.handle, image_handle);
  }
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle));
}

TEST_F(FlatlandTest, SetTheSameContentOnMultipleTransforms) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto import_token_dup = ref_pair.DuplicateImportToken();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));

  // Create a transform, make it the root transform, and add two children.
  const TransformId kTransformId1(1);
  const TransformId kTransformId2(2);
  const TransformId kTransformId3(3);

  flatland->CreateTransform(kTransformId1);
  flatland->CreateTransform(kTransformId2);
  flatland->CreateTransform(kTransformId3);

  flatland->SetRootTransform(kTransformId1);
  flatland->AddChild(kTransformId1, kTransformId2);
  flatland->AddChild(kTransformId1, kTransformId3);

  // Set the same content on both children
  flatland->SetContent(kTransformId2, kImageId);
  flatland->SetContent(kTransformId3, kImageId);
  Present(flatland, true);
}

TEST_F(FlatlandTest, TopologyVisitsContentBeforeChildren) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup two valid images.
  const ContentId kImageId1(1);
  auto ref_pair_1 = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  CreateImage(flatland.get(), allocator.get(), kImageId1, std::move(ref_pair_1),
              std::move(properties1));

  const auto maybe_image_handle1 = flatland->GetContentHandle(kImageId1);
  ASSERT_TRUE(maybe_image_handle1.has_value());
  const auto image_handle1 = maybe_image_handle1.value();

  const ContentId kImageId2(2);
  auto ref_pair_2 = BufferCollectionImportExportTokens::New();

  ImageProperties properties2;
  properties2.size(SizeU{300, 400});

  CreateImage(flatland.get(), allocator.get(), kImageId2, std::move(ref_pair_2),
              std::move(properties2));

  const auto maybe_image_handle2 = flatland->GetContentHandle(kImageId2);
  ASSERT_TRUE(maybe_image_handle2.has_value());
  const auto image_handle2 = maybe_image_handle2.value();

  // Create a root transform with two children.
  const TransformId kTransformId1(3);
  const TransformId kTransformId2(4);
  const TransformId kTransformId3(5);

  flatland->CreateTransform(kTransformId1);
  flatland->CreateTransform(kTransformId2);
  flatland->CreateTransform(kTransformId3);

  flatland->AddChild(kTransformId1, kTransformId2);
  flatland->AddChild(kTransformId1, kTransformId3);

  flatland->SetRootTransform(kTransformId1);
  Present(flatland, true);

  // Attach image 1 to the root and the second child-> Attach image 2 to the first child->
  flatland->SetContent(kTransformId1, kImageId1);
  flatland->SetContent(kTransformId2, kImageId2);
  flatland->SetContent(kTransformId3, kImageId1);
  Present(flatland, true);

  // The images should appear pre-order toplogically sorted: 1, 2, 1 again. The same image is
  // allowed to appear multiple times.
  std::queue<TransformHandle> expected_handle_order;
  expected_handle_order.push(image_handle1);
  expected_handle_order.push(image_handle2);
  expected_handle_order.push(image_handle1);
  auto uber_struct = GetUberStruct(flatland.get());
  for (const auto& entry : uber_struct->local_topology) {
    if (entry.handle == expected_handle_order.front()) {
      expected_handle_order.pop();
    }
  }
  EXPECT_TRUE(expected_handle_order.empty());

  // Clearing the image from the parent removes the first entry of the list since images are
  // visited before children.
  flatland->SetContent(kTransformId1, kInvalidContentId);
  Present(flatland, true);

  // Meaning the new list of images should be: 2, 1.
  expected_handle_order.push(image_handle2);
  expected_handle_order.push(image_handle1);
  uber_struct = GetUberStruct(flatland.get());
  for (const auto& entry : uber_struct->local_topology) {
    if (entry.handle == expected_handle_order.front()) {
      expected_handle_order.pop();
    }
  }
  EXPECT_TRUE(expected_handle_order.empty());
}

// Tests that a buffer collection is released after CreateImage() if there are no more import
// tokens.
TEST_F(FlatlandTest, ReleaseBufferCollectionHappensAfterCreateImage) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Register a valid buffer collection.
  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ContentId kImageId(1);
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();

  // Send our only import token to CreateImage(). Buffer collection should be released only after
  // Image creation.
  {
    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(_, _)).Times(1);
    flatland->CreateImage(kImageId, ToWire(std::move(ref_pair.import_token)), 0, properties);
    RunLoopUntilIdle();
  }
}

// In this variation, the image is explicitly released, and the internally-generated release fence
// is signaled after because we don't set `args.skip_session_update_and_release_fences` as we do
// in the other variations.
TEST_F(FlatlandTest, ReleaseBufferCollectionCompletesAfterFlatlandDestruction1) {
  allocation::GlobalBufferCollectionId global_collection_id;
  display::ImageId global_image_id;
  {
    std::shared_ptr<Allocator> allocator = CreateAllocator();
    std::shared_ptr<Flatland> flatland = CreateFlatland();

    const ContentId kImageId(3);
    auto ref_pair = BufferCollectionImportExportTokens::New();
    ImageProperties properties;
    properties.size(SizeU{200, 200});
    auto import_token_dup = ref_pair.DuplicateImportToken();
    auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                      std::move(ref_pair), std::move(properties));
    global_collection_id = global_id_pair.collection_id;
    global_image_id = global_id_pair.image_id;

    // Release the image.
    flatland->ReleaseImage(kImageId);

    // Release the buffer collection.

    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
        .Times(1);
    import_token_dup.value().reset();
    RunLoopUntilIdle();

    // Present, apply updates, and signal fences so that we can verify that the image is released.
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
    {
      PresentWithArgs(flatland, PresentArgs{}, true);
    }
    RunLoopUntilIdle();

    // |flatland| is about to fall out of scope.  Ensure that no image is released, because it
    // always was earlier..
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(0);
  }
}

// In this variation, the image is explicitly released, but the BufferCollectionImporter isn't
// notified until the Flatland session is destroyed.  This exercises the logic in ~Flatland(), and
// verifies that we don't call ReleaseBufferImage() twice for the same image.
TEST_F(FlatlandTest, ReleaseBufferCollectionCompletesAfterFlatlandDestruction2) {
  allocation::GlobalBufferCollectionId global_collection_id;
  display::ImageId global_image_id;
  {
    std::shared_ptr<Allocator> allocator = CreateAllocator();
    std::shared_ptr<Flatland> flatland = CreateFlatland();

    const ContentId kImageId(3);
    auto ref_pair = BufferCollectionImportExportTokens::New();
    ImageProperties properties;
    properties.size(SizeU{200, 200});
    auto import_token_dup = ref_pair.DuplicateImportToken();
    auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                      std::move(ref_pair), std::move(properties));
    global_collection_id = global_id_pair.collection_id;
    global_image_id = global_id_pair.image_id;

    // Release the image.
    flatland->ReleaseImage(kImageId);

    // Release the buffer collection.

    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
        .Times(1);
    import_token_dup.value().reset();
    RunLoopUntilIdle();

    // Skip session updates to test that release fences are what trigger the importer calls.
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(0);
    PresentArgs args;
    args.skip_session_update_and_release_fences = true;
    {
      PresentWithArgs(flatland, std::move(args), true);
    }

    // |flatland| is about to fall out of scope.  Ensure that the images is released.
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  }
}

// This variation is similar to ReleaseBufferCollectionCompletesAfterFlatlandDestruction1, except
// that the image has not been released by the client by the time that the Flatland session is
// destroyed.
TEST_F(FlatlandTest, ReleaseBufferCollectionCompletesAfterFlatlandDestruction3) {
  allocation::GlobalBufferCollectionId global_collection_id;
  display::ImageId global_image_id;
  {
    std::shared_ptr<Allocator> allocator = CreateAllocator();
    std::shared_ptr<Flatland> flatland = CreateFlatland();

    const ContentId kImageId(3);
    auto ref_pair = BufferCollectionImportExportTokens::New();
    ImageProperties properties;
    properties.size(SizeU{200, 200});
    auto import_token_dup = ref_pair.DuplicateImportToken();
    auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                      std::move(ref_pair), std::move(properties));
    global_collection_id = global_id_pair.collection_id;
    global_image_id = global_id_pair.image_id;

    // Release the buffer collection.

    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
        .Times(1);
    import_token_dup.value().reset();
    RunLoopUntilIdle();

    // Skip session updates to test that release fences are what trigger the importer calls.
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(0);
    PresentArgs args;
    args.skip_session_update_and_release_fences = true;
    {
      PresentWithArgs(flatland, std::move(args), true);
    }

    // |flatland| is about to fall out of scope.  Ensure that the image is released.
    EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  }
}

// Tests that an Image is not released from the importer until it is not referenced and the
// release fence is signaled.
TEST_F(FlatlandTest, ReleaseImageWaitsForReleaseFence) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid buffer collection and Image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties;
  properties.size(SizeU{100, 200});

  auto import_token_dup = ref_pair.DuplicateImportToken();
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto& global_collection_id = global_id_pair.collection_id;

  // Attach the Image to a transform.
  const TransformId kTransformId(3);
  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);

  // Release the buffer collection, but ensure that the ReleaseBufferImage call on the importer
  // has not happened.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
      .Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  import_token_dup.value().reset();
  RunLoopUntilIdle();

  // Release the Image that referenced the buffer collection. Because the Image is still attached
  // to a Transform, the deregestration call should still not happen.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Remove the Image from the transform. This triggers the creation of the release fence, but
  // still does not result in a deregestration call. Skip session updates to test that release
  // fences are what trigger the importer calls.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->SetContent(kTransformId, kInvalidContentId);

  PresentArgs args;
  args.skip_session_update_and_release_fences = true;
  PresentWithArgs(flatland, std::move(args), true);

  // Signal the release fences, which triggers the release call.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, ReleaseImageErrorCases) {
  // Zero is not a valid image ID.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->ReleaseImage(kInvalidContentId);
    Present(flatland, false);
  }

  // The image must exist.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->ReleaseImage(ContentId(1));
    Present(flatland, false);
  }

  // ContentId is not an Image.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    ViewportCreationToken parent_token;
    ViewCreationToken child_token;
    ASSERT_EQ(ZX_OK, zx::channel::create(0, &parent_token.value(), &child_token.value()));

    const ContentId kLinkId(2);

    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::Endpoints<ChildViewWatcher>::Create();
    fidl::Arena arena;
    auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                          .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                          .Build();
    flatland->CreateViewport(kLinkId, ToWire(std::move(parent_token)), properties,
                             std::move(child_view_watcher_server_end));

    flatland->ReleaseImage(kLinkId);
    Present(flatland, false);
  }
}

// One importer imports the image and the other fails.  The failed CreateImage() drops the
// layer's ref, and the image is released from every importer exactly once, through the
// session-teardown fence: once on the importer that succeeded, and once on the importer that
// failed, for which the release is a no-op (see BufferCollectionImporter::ReleaseBufferImage).
TEST_F(FlatlandTest, ImportFailureOnOneImporterReleasesEveryImporterOnce) {
  // Create a second buffer collection importer.
  auto local_mock_buffer_collection_importer = new MockBufferCollectionImporter();
  auto local_buffer_collection_importer =
      std::shared_ptr<allocation::BufferCollectionImporter>(local_mock_buffer_collection_importer);

  // Create flatland and allocator instances that have two BufferCollectionImporters.
  std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers(
      {buffer_collection_importer_, local_buffer_collection_importer});
  std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> screenshot_importers;
  std::shared_ptr<Allocator> allocator = std::make_shared<Allocator>(
      dispatcher(), context_provider_.context(), importers, screenshot_importers,
      utils::CreateSysmemAllocatorClient(dispatcher(), "ImportFailureReleasesEveryImporterOnce"));
  auto session_id = scheduling::GetNextSessionId();

  std::optional<std::string> error_log;
  auto [flatland_client_end, flatland_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();
  auto flatland = Flatland::New(
      std::make_shared<utils::UnownedDispatcherHolder>(dispatcher()),
      std::move(flatland_server_end), session_id,
      /*destroy_instance_functon=*/[]() {}, flatland_presenter_, link_system_,
      uber_struct_system_->AllocateQueueForSession(session_id), importers, [](auto...) {},
      [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {},
      FlatlandConfig{.use_trusted_flatland_api = true});
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  // Wait for Bind() to occur within Flatland::New().
  RunLoopUntilIdle();

  EXPECT_CALL(*local_mock_buffer_collection_importer, ImportBufferCollection(_, _, _, _, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();

  allocation::GlobalImageId image_id = allocation::kInvalidImageId;
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce([&image_id](const allocation::ImageMetadata& metadata,
                            allocation::BufferCollectionUsage) {
        image_id = metadata.identifier;
        return fpromise::make_ok_promise();
      });
  EXPECT_CALL(*local_mock_buffer_collection_importer, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::error()));

  flatland->CreateImage(ContentId(1), ToWire(std::move(ref_pair.import_token)), /*vmo_idx*/ 0,
                        properties);
  RunLoopUntilIdle();
  ASSERT_TRUE(error_log.has_value());
  ASSERT_NE(image_id, allocation::kInvalidImageId);

  // Nothing has been released yet.  The id rides the session-teardown fence, which the fixture's
  // default RemoveSession() action signals.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(image_id)).Times(1);
  EXPECT_CALL(*local_mock_buffer_collection_importer, ReleaseBufferImage(image_id)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

// Test to make sure that if a buffer collection importer returns |false|
// on |ImportBufferImage()| that this is caught when we try to present.
TEST_F(FlatlandTest, BufferImporterImportImageReturnsFalseTest) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  // Create a proper properties struct.
  fidl::Arena arena1;
  auto properties1 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena1)
                         .size(fuchsia_math::wire::SizeU{150, 175})
                         .Build();

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  // We've imported a proper image and we have the importer returning true, so
  // Present() should return true.
  flatland->CreateImage(ContentId(1), ToWire(ref_pair.DuplicateImportToken()), /*vmo_idx*/ 0,
                        properties1);
  RunLoopUntilIdle();
  Present(flatland, true);

  // We're using the same buffer collection so we don't need to validate, only import.
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::error()));

  // Import again, but this time have the importer return false. Flatland should catch
  // this and Present() should return false.
  fidl::Arena arena2;
  auto properties2 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena2)
                         .size(fuchsia_math::wire::SizeU{150, 175})
                         .Build();
  flatland->CreateImage(ContentId(2), ToWire(ref_pair.DuplicateImportToken()), /*vmo_idx*/ 0,
                        properties2);
  RunLoopUntilIdle();
  Present(flatland, false);
}

// Test to make sure that the release fences signal to the buffer importer
// to release the image.
TEST_F(FlatlandTest, BufferImporterImageReleaseTest) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  const allocation::GlobalBufferCollectionId global_collection_id1 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
                  std::move(properties1))
          .collection_id;

  // Create a transform, make it the root transform, and attach the image.
  const TransformId kTransformId(2);

  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);

  // Now release the image.
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Now remove the image from the transform, which should result in it being
  // garbage collected.
  flatland->SetContent(kTransformId, kInvalidContentId);
  PresentArgs args;
  args.skip_session_update_and_release_fences = true;
  PresentWithArgs(flatland, std::move(args), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, ReleasedImageRemainsUntilCleared) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  const allocation::GlobalBufferCollectionId global_collection_id =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
                  std::move(properties1))
          .collection_id;

  const auto maybe_image_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_image_handle.has_value());
  const auto image_handle = maybe_image_handle.value();

  // Create a transform, make it the root transform, and attach the image.
  const TransformId kTransformId(2);

  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);

  // The image handle should be the last handle in the local_topology, and the image should be in
  // the image map.
  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_EQ(uber_struct->local_topology.back().handle, image_handle);
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle));

  // Releasing the image succeeds, but all data remains in the UberStruct.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  EXPECT_EQ(uber_struct->local_topology.back().handle, image_handle);
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle));

  // Clearing the Transform of its Image detaches it, triggering the release.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(1);
  flatland->SetContent(kTransformId, kInvalidContentId);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  for (const auto& entry : uber_struct->local_topology) {
    EXPECT_NE(entry.handle, image_handle);
  }
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle));
}

TEST_F(FlatlandTest, ReleasedImageIdCanBeReused) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair_1 = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  const allocation::GlobalBufferCollectionId global_collection_id1 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair_1),
                  std::move(properties1))
          .collection_id;

  const auto maybe_image_handle1 = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_image_handle1.has_value());
  const auto image_handle1 = maybe_image_handle1.value();

  // Create a transform, make it the root transform, attach the image, then release it.
  const TransformId kTransformId1(2);

  flatland->CreateTransform(kTransformId1);
  flatland->SetRootTransform(kTransformId1);
  flatland->SetContent(kTransformId1, kImageId);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle1));

  // The ContentId can be reused even though the old image is still present. Add a second
  // transform so that both images show up in the global image vector.
  auto ref_pair_2 = BufferCollectionImportExportTokens::New();
  ImageProperties properties2;
  properties2.size(SizeU{300, 400});

  const allocation::GlobalBufferCollectionId global_collection_id2 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair_2),
                  std::move(properties2))
          .collection_id;

  const TransformId kTransformId2(3);

  flatland->CreateTransform(kTransformId2);
  flatland->AddChild(kTransformId1, kTransformId2);
  flatland->SetContent(kTransformId2, kImageId);
  // Both images are attached, so releasing the first image ID shouldn't have released it from the
  // importer.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  const auto maybe_image_handle2 = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_image_handle2.has_value());
  const auto image_handle2 = maybe_image_handle2.value();

  // Both images should appear in the image map.
  uber_struct = GetUberStruct(flatland.get());
  EXPECT_NE(image_handle1, image_handle2);
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle1));
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle2));

  // Both images should be attached to the topology.
  bool found_handle1 = false;
  bool found_handle2 = false;
  for (const auto& entry : uber_struct->local_topology) {
    if (entry.handle == image_handle1)
      found_handle1 = true;
    if (entry.handle == image_handle2)
      found_handle2 = true;
  }
  EXPECT_TRUE(found_handle1);
  EXPECT_TRUE(found_handle2);

  RunLoopUntilIdle();
  testing::Mock::VerifyAndClearExpectations(mock_buffer_collection_importer_);
}

// Test that released Images, when attached to a Transform, are not garbage collected even if
// the Transform is not part of the most recently presented global topology.
TEST_F(FlatlandTest, ReleasedImagePersistsOutsideGlobalTopology) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  const allocation::GlobalBufferCollectionId global_collection_id1 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
                  std::move(properties1))
          .collection_id;

  const auto maybe_image_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_image_handle.has_value());
  const auto image_handle = maybe_image_handle.value();

  // Create a transform, make it the root transform, attach the image, then release it.
  const TransformId kTransformId(2);

  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Remove the entire hierarchy. It is detached from the root, but not cleared
  // from the transform, so it should not be released.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->SetRootTransform(kInvalidTransformId);
  Present(flatland, true);
  RunLoopUntilIdle();
  testing::Mock::VerifyAndClearExpectations(mock_buffer_collection_importer_);

  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle));

  // Reintroduce the hierarchy and confirm the Image is still present, even though it was
  // temporarily not reachable from the root transform.
  flatland->SetRootTransform(kTransformId);
  Present(flatland, true);

  uber_struct = GetUberStruct(flatland.get());
  EXPECT_EQ(uber_struct->local_topology.back().handle, image_handle);
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle));
}

TEST_F(FlatlandTest, ClearReleasesImagesAndBufferCollections) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid image.
  const ContentId kImageId(1);
  auto ref_pair_1 = BufferCollectionImportExportTokens::New();

  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  auto import_token_dup = ref_pair_1.DuplicateImportToken();
  const allocation::GlobalBufferCollectionId global_collection_id1 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair_1),
                  std::move(properties1))
          .collection_id;

  // Create a transform, make it the root transform, and attach the Image.
  const TransformId kTransformId(2);

  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);

  // Clear the graph, then signal the release fence and ensure the buffer collection is released.
  flatland->Clear();
  import_token_dup.value().reset();

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id1, _))
      .Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(display::ImageId(1))).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(display::ImageId(2))).Times(1);

  Present(flatland, true);

  // The Image ID should be available for re-use.
  auto ref_pair_2 = BufferCollectionImportExportTokens::New();
  ImageProperties properties2;
  properties2.size(SizeU{400, 800});

  const allocation::GlobalBufferCollectionId global_collection_id2 =
      CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair_2),
                  std::move(properties2))
          .collection_id;

  EXPECT_NE(global_collection_id1, global_collection_id2);

  // Verify that the Image is valid and can be attached to a transform.
  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId);
  Present(flatland, true);
}

TEST_F(FlatlandTest, BufferSwapReleasesPreviousImage) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup two valid images.
  const ContentId kImageId1(1);
  auto ref_pair_1 = BufferCollectionImportExportTokens::New();
  ImageProperties properties1;
  properties1.size(SizeU{100, 200});

  const allocation::GlobalBufferCollectionId global_collection_id1 =
      CreateImage(flatland.get(), allocator.get(), kImageId1, std::move(ref_pair_1),
                  std::move(properties1))
          .collection_id;

  const auto maybe_image_handle1 = flatland->GetContentHandle(kImageId1);
  ASSERT_TRUE(maybe_image_handle1.has_value());
  const auto image_handle1 = maybe_image_handle1.value();

  const ContentId kImageId2(2);
  auto ref_pair_2 = BufferCollectionImportExportTokens::New();
  ImageProperties properties2;
  properties2.size(SizeU{300, 400});

  const allocation::GlobalBufferCollectionId global_collection_id2 =
      CreateImage(flatland.get(), allocator.get(), kImageId2, std::move(ref_pair_2),
                  std::move(properties2))
          .collection_id;

  const auto maybe_image_handle2 = flatland->GetContentHandle(kImageId2);
  ASSERT_TRUE(maybe_image_handle2.has_value());
  const auto image_handle2 = maybe_image_handle2.value();

  // Create a transform, make it the root transform, and attach the first image.
  const TransformId kTransformId(3);
  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);
  flatland->SetContent(kTransformId, kImageId1);
  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle1));
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle2));

  // Releasing image 1 succeeds, but since it is still attached to the transform,
  // it is not yet released from the importer.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId1);
  Present(flatland, true);
  RunLoopUntilIdle();
  testing::Mock::VerifyAndClearExpectations(mock_buffer_collection_importer_);

  uber_struct = GetUberStruct(flatland.get());
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle1));
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle2));

  // Now swap the content on the transform to image 2. This detaches image 1,
  // which should trigger its release.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(1);
  flatland->SetContent(kTransformId, kImageId2);
  Present(flatland, true);
  RunLoopUntilIdle();
  testing::Mock::VerifyAndClearExpectations(mock_buffer_collection_importer_);

  // Verify that the topology now has image 2's handle, and no longer contains image 1's handle.
  uber_struct = GetUberStruct(flatland.get());
  EXPECT_FALSE(uber_struct->HasLayerContentForTest(image_handle1));
  EXPECT_TRUE(uber_struct->HasLayerContentForTest(image_handle2));
  EXPECT_EQ(uber_struct->local_topology.back().handle, image_handle2);
  for (const auto& entry : uber_struct->local_topology) {
    EXPECT_NE(entry.handle, image_handle1);
  }
}

TEST_F(FlatlandTest, UnsquashableUpdates_ShouldBeReflectedInScheduleUpdates) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // We call Present() twice, each time passing a different value as the unsquashable argument.
  // We EXPECT that the ensuing ScheduleUpdateForSession() call to the frame scheduler will
  // reflect the passed in squashable value.

  // Present with the unsquashable field set to true.
  {
    PresentArgs args;
    args.unsquashable = true;
    PresentWithArgs(flatland, std::move(args), true);
  }

  // Present with the unsquashable field set to false.
  {
    PresentArgs args;
    args.unsquashable = false;
    PresentWithArgs(flatland, std::move(args), true);
  }
}

TEST_F(FlatlandTest, MultithreadedLinkResolution) {
  auto parent_flatland = CreateFlatland();

  std::shared_ptr<Flatland> child_flatland;
  auto child_flatland_thread_loop_holder =
      std::make_shared<utils::LoopDispatcherHolder>(&kAsyncLoopConfigNoAttachToCurrentThread);
  auto& child_flatland_thread_loop = child_flatland_thread_loop_holder->loop();
  auto [child_flatland_client_end, child_flatland_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();

  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<ChildViewWatcher>::Create();
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end_workaround] =
      fidl::Endpoints<ParentViewportWatcher>::Create();
  // C++-20 workaround, so that this can be captured in the closure below.
  fidl::ServerEnd<ParentViewportWatcher> parent_viewport_watcher_server_end(
      std::move(parent_viewport_watcher_server_end_workaround));

  {
    auto session_id = scheduling::GetNextSessionId();
    std::vector<std::shared_ptr<BufferCollectionImporter>> importers;
    importers.push_back(buffer_collection_importer_);

    child_flatland = Flatland::New(
        child_flatland_thread_loop_holder, std::move(child_flatland_server_end), session_id,
        [](auto...) {}, flatland_presenter_, link_system_,
        uber_struct_system_->AllocateQueueForSession(session_id), importers, [](auto...) {},
        [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {},
        FlatlandConfig{.use_trusted_flatland_api = true});

    auto status = child_flatland_thread_loop.StartThread();
    EXPECT_EQ(status, ZX_OK);
  }

  auto creation_tokens = scenic::cpp::ViewCreationTokenPair::New();
  const TransformId kRootTransform(1);
  const ContentId kLinkId(1);

  // One of the link ends needs to run first.  Here, we arbitrarily choose it to be the viewport
  // end, and wait for it to finish.  Of course, the link is not yet resolved, because the view
  // hasn't yet been created.
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ViewportProperties::Builder(arena)
                        .logical_size(fuchsia_math::wire::SizeU{kDefaultSize, kDefaultSize})
                        .Build();
  parent_flatland->CreateTransform(kRootTransform);
  parent_flatland->SetRootTransform(kRootTransform);
  parent_flatland->CreateViewport(kLinkId, ToWire(std::move(creation_tokens.viewport_token)),
                                  properties, std::move(child_view_watcher_server_end));
  parent_flatland->SetContent(kRootTransform, kLinkId);
  Present(parent_flatland, true);
  RunLoopUntilIdle();

  ApplySessionUpdatesAndSignalFences();
  UpdateLinks(parent_flatland->GetRoot());
  auto links = link_system_->GetResolvedTopologyLinks();
  EXPECT_EQ(links.size(), 0U);

  // We post this task onto the other Flatland's thread, so that we can have a *chance* of locking
  // the LinkSystem mutex in between the execution of the two link-resolution closures (each of
  // which also locks the LinkSystem mutex).
  EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _));
  libsync::Completion completion;
  async::PostTask(child_flatland_thread_loop.dispatcher(), ([&]() {
                    child_flatland->CreateView2(ToWire(std::move(creation_tokens.view_token)),
                                                NewWireViewIdentityOnCreation(), NoViewProtocols(),
                                                std::move(parent_viewport_watcher_server_end));

                    fuchsia_ui_composition::wire::PresentArgs present_args;
                    // `Present()` puts the resulting UberStruct into the "acquire fence queue";
                    // since there are no fences it is not-quite-immediately made available via a
                    // posted task.
                    child_flatland->Present(present_args);
                    completion.Signal();
                  }));

  // This runs "concurrently" with the task above.  Ideally, it will sometimes fall between the
  // running the two ObjectLinker link-resolved closures.  Unfortunately this is unlikely, so we
  // can't reliably ensure that this case is handled properly.  More often, this will occur either
  // before both link-resolved closures, or after both of them (both of these situations are
  // handled properly).
  ApplySessionUpdatesAndSignalFences();
  UpdateLinks(parent_flatland->GetRoot());
  links = link_system_->GetResolvedTopologyLinks();
  // One of these will be true, but we don't know which one.
  // EXPECT_EQ(links.size(), 0U);
  // EXPECT_EQ(links.size(), 1U);

  // By waiting for the task to finish running, we know that the Present() has happened, and
  // therefore both the parent and child Flatland sessions will have provided UberStructs, so
  // that there will now be a resolved link between the viewport and the view.
  completion.Wait();

  ApplySessionUpdatesAndSignalFences();
  UpdateLinks(parent_flatland->GetRoot());
  links = link_system_->GetResolvedTopologyLinks();
  EXPECT_EQ(links.size(), 1U);

  // Need to destroy the child Flatland on the correct thread, so the FIDL binding doesn't CHECK.
  async::PostTask(child_flatland_thread_loop.dispatcher(),
                  [&, flatland_to_destroy = std::move(child_flatland)]() {
                    child_flatland_thread_loop.Quit();
                  });
  child_flatland_thread_loop.JoinThreads();
}

// Verify that `destroy_instance_function_()` is only invoked once, even if there are multiple
// errors that each would alone cause `destroy_instance_function_()` to be invoked.
TEST_F(FlatlandTest, NoDoubleDestroyRequest) {
  // Create flatland and allocator instances that has two BufferCollectionImporters.
  std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> no_importers;
  std::shared_ptr<Allocator> allocator = std::make_shared<Allocator>(
      dispatcher(), context_provider_.context(), no_importers, no_importers,
      utils::CreateSysmemAllocatorClient(dispatcher(), "NoDoubleDestroyRequest"));

  auto session_id = scheduling::GetNextSessionId();

  size_t destroy_instance_function_invocation_count = 0;
  auto [flatland_client_end, flatland_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();
  auto flatland = Flatland::New(
      std::make_shared<utils::UnownedDispatcherHolder>(dispatcher()),
      std::move(flatland_server_end), session_id,
      /*destroy_instance_functon=*/
      [&destroy_instance_function_invocation_count]() {
        ++destroy_instance_function_invocation_count;
      },
      flatland_presenter_, link_system_, uber_struct_system_->AllocateQueueForSession(session_id),
      no_importers, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {}, [](auto...) {},
      [](auto...) {}, FlatlandConfig{.use_trusted_flatland_api = true});

  // Wait for server channel to be bound; see `Flatland::Bind()`.
  RunLoopUntilIdle();

  fuchsia_ui_composition::wire::PresentArgs present_args;

  flatland->AddChild(TransformId(11), TransformId(12));
  flatland->Present(present_args);

  EXPECT_EQ(destroy_instance_function_invocation_count, 1U);

  flatland->AddChild(TransformId(11), TransformId(12));
  flatland->Present(present_args);

  // If it wasn't for the guard variable `destroy_instance_function_was_invoked_` in
  // `Flatland::CloseConnection()`, this check would fail deterministically.
  EXPECT_EQ(destroy_instance_function_invocation_count, 1U);
}

TEST_F(FlatlandTest, CreateAndReleaseFilledRect) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _)).Times(0);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(allocation::kInvalidImageId))
      .Times(0);
  const ContentId kFilledRectId(1);
  flatland->CreateFilledRect(kFilledRectId);
  Present(flatland, true);
  flatland->ReleaseFilledRect(kFilledRectId);
  Present(flatland, true);
}

TEST_F(FlatlandTest, PresentWithScheduleAsapConfig) {
  FlatlandConfig config{.schedule_asap = true};
  std::shared_ptr<Flatland> flatland = CreateFlatland(std::move(config));

  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(_, _, _, _, _, _, /*schedule_asap=*/true));

  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  RunLoopUntilIdle();
  ApplySessionUpdatesAndSignalFences();
}

TEST_F(FlatlandDisplayTest, SimpleSetContent) {
  constexpr uint32_t kWidth = 800;
  constexpr uint32_t kHeight = 600;

  std::shared_ptr<FlatlandDisplay> display = CreateFlatlandDisplay(kWidth, kHeight);

  std::shared_ptr<Flatland> child = CreateFlatland();
  ConnectChildViewToDisplayThenValidate(display, child, kWidth, kHeight);

  std::shared_ptr<Flatland> child2 = CreateFlatland();
  ConnectChildViewToDisplayThenValidate(display, child2, kWidth, kHeight);

  std::shared_ptr<Flatland> child3 = CreateFlatland();
  ConnectChildViewToDisplayThenValidate(display, child3, kWidth, kHeight);

  // Verify that previously-connected Flatland sessions can re-connect to the display.
  ConnectChildViewToDisplayThenValidate(display, child2, kWidth, kHeight);
  ConnectChildViewToDisplayThenValidate(display, child, kWidth, kHeight);
}

// TODO(https://fxbug.dev/42156567): other FlatlandDisplayTests that should be written:
// - version of SimpleSetContent where the child presents before SetDisplayContent() is called.

TEST_F(FlatlandTest, ReleaseImageImmediatelyUntrusted) {
  // Default CreateFlatland() creates an untrusted session.
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Calling ReleaseImageImmediately on an untrusted session should close the connection.
  const ContentId kImageId(3);
  flatland->ReleaseImageImmediately(kImageId);

  RunLoopUntilIdle();

  // Verify the connection was closed with kBadOperation.
  EXPECT_EQ(GetFlatlandError(flatland->GetSessionId()), FlatlandError::kBadOperation);
}

TEST_F(FlatlandTest, ReleaseImageImmediatelyTrusted) {
  // Create a trusted session.
  FlatlandConfig config{.use_trusted_flatland_api = true};
  std::shared_ptr<Flatland> flatland = CreateFlatland(std::move(config));

  // Create an image to release.
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  const ContentId kImageId(1);
  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  const auto global_image_id = global_id_pair.image_id;
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id), nullptr);

  // Verify the image is present.
  EXPECT_TRUE(flatland->GetContentHandle(kImageId).has_value());

  // Calling ReleaseImageImmediately on a trusted session should succeed (one-way call).
  flatland->ReleaseImageImmediately(kImageId);

  // Verify ImageObject is gone right after the call without needing Present().
  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id), nullptr);

  RunLoopUntilIdle();

  // Verify the image is gone.
  EXPECT_FALSE(flatland->GetContentHandle(kImageId).has_value());
}

TEST_F(FlatlandTest, CreateLayerRejectsZeroId) {
  std::shared_ptr<Flatland> flatland = CreateFlatland(FlatlandConfig{.use_flatland2 = true});
  flatland->CreateLayer(kInvalidLayerId);
  Present(flatland, false);
}

TEST_F(FlatlandTest, SetTransformContentRejectsZeroId) {
  const TransformId kId(1);

  // LayerStack variant.
  {
    std::optional<std::string> error_log;
    std::shared_ptr<Flatland> flatland = CreateFlatland(FlatlandConfig{.use_flatland2 = true});
    flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
    flatland->CreateTransform(kId);
    fidl::Arena arena;
    auto content = fuchsia_ui_composition::wire::TransformContent::WithLayerStack(
        arena, fuchsia_ui_composition::wire::LayerStackId{0});
    flatland->SetTransformContent(kId, &content);
    Present(flatland, false);
    ASSERT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("must be non-zero"), std::string::npos);
  }

  // Viewport variant.
  {
    std::optional<std::string> error_log;
    std::shared_ptr<Flatland> flatland = CreateFlatland(FlatlandConfig{.use_flatland2 = true});
    flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
    flatland->CreateTransform(kId);
    fidl::Arena arena;
    auto content = fuchsia_ui_composition::wire::TransformContent::WithViewport(
        arena, fuchsia_ui_composition::wire::ViewportId{0});
    flatland->SetTransformContent(kId, &content);
    Present(flatland, false);
    ASSERT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("must be non-zero"), std::string::npos);
  }
}

TEST_F(FlatlandTest, SetStackLayersRejectsTooManyLayers) {
  std::optional<std::string> error_log;
  std::shared_ptr<Flatland> flatland = CreateFlatland(FlatlandConfig{.use_flatland2 = true});
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  const std::vector<LayerId> layers(fuchsia_ui_composition::kMaxStackLayers + 1, LayerId(1));
  flatland->SetStackLayers(LayerStackId(1), layers);
  Present(flatland, false);
  ASSERT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("too many layers"), std::string::npos);
}

TEST_F(FlatlandTest, LayerHandleNeverReused) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const size_t N = 10;
  std::vector<LayerHandle> first_batch;
  for (size_t i = 0; i < N; ++i) {
    auto handle = flatland->CreateLayerObject();
    flatland->GetLayerObjectForTest(handle)->ref_count = 1;
    first_batch.push_back(handle);
  }

  for (auto handle : first_batch) {
    flatland->ReleaseLayerObject(handle);
  }

  std::vector<LayerHandle> second_batch;
  for (size_t i = 0; i < N; ++i) {
    second_batch.push_back(flatland->CreateLayerObject());
  }

  // Verify all 2N handles are distinct.
  std::unordered_set<LayerHandle> all_handles;
  for (auto handle : first_batch) {
    all_handles.insert(handle);
  }
  for (auto handle : second_batch) {
    all_handles.insert(handle);
  }
  EXPECT_EQ(all_handles.size(), 2 * N);
}

TEST_F(FlatlandTest, DistinctFromTransformHandles) {
  static_assert(!std::is_convertible_v<TransformHandle, LayerHandle>);
  static_assert(!std::is_convertible_v<LayerHandle, TransformHandle>);
}

TEST_F(FlatlandTest, LayerSurvivesWhileStackReferencesIt) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create layer.
  LayerHandle layer = flatland->CreateLayerObject();

  // Create stack.
  auto stack_content_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_content_handle, {layer});

  // Verify layer object is still present (ref_count is 1 from the stack).
  const auto* obj = flatland->GetLayerObjectForTest(layer);
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->ref_count, 1);

  // Release the stack content handle from the transform graph.
  flatland->ReleaseTransformForTest(stack_content_handle);

  // Invoke the Flatland2 GC manually.
  flatland->CleanupFlatland2StateForTest({stack_content_handle});

  // Now both should be gone.
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer), nullptr);
}

TEST_F(FlatlandTest, LayerDiesImmediatelyAfterLastRef) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  LayerHandle layer = flatland->CreateLayerObject();
  flatland->GetLayerObjectForTest(layer)->ref_count = 1;

  // Release layer.
  flatland->ReleaseLayerObject(layer);

  // Gone immediately.
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer), nullptr);
}

TEST_F(FlatlandTest, StackKeepAliveViaAttachedTransform) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create transform and set it as root.
  const TransformId kTransformId(3);
  flatland->CreateTransform(kTransformId);
  flatland->SetRootTransform(kTransformId);

  LayerHandle layer = flatland->CreateLayerObject();
  auto stack_content_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_content_handle, {layer});

  // Attach stack to the transform (using the content handle).
  flatland->SetPriorityChildForTest(kTransformId, stack_content_handle);

  // Present to commit the attachment.
  Present(flatland, true);

  // Release the stack. It should be kept alive because it is attached to the transform.
  flatland->ReleaseTransformForTest(stack_content_handle);
  flatland->CleanupFlatland2StateForTest({});

  // Layer should still be alive because stack is alive.
  const auto* obj = flatland->GetLayerObjectForTest(layer);
  ASSERT_NE(obj, nullptr);

  // Now detach the stack from the transform.
  flatland->SetPriorityChildForTest(kTransformId, TransformHandle());

  // Invoke the Flatland2 GC manually, passing the now dead stack_content_handle.
  flatland->CleanupFlatland2StateForTest({stack_content_handle});

  // The stack's content handle is now dead, so the stack is deleted, and the layer is GC'd.
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer), nullptr);
}

TEST_F(FlatlandTest, ImageReleaseRidesExistingMachinery) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Setup a valid buffer collection and Image.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties;
  properties.size(SizeU{100, 200});

  auto import_token_dup = ref_pair.DuplicateImportToken();
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto& global_collection_id = global_id_pair.collection_id;
  auto global_image_id = global_id_pair.image_id;

  // Create layer and bind image.
  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id);

  // Put in a stack.
  auto stack_content_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_content_handle, {layer});

  // Release the buffer collection.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
      .Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  import_token_dup.value().reset();
  RunLoopUntilIdle();

  // Release the classic image. Since it is bound to our live layer, it should NOT be released yet.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Now release the stack (by making it dead).
  flatland->ReleaseTransformForTest(stack_content_handle);

  // Invoke the Flatland2 GC manually.
  auto released_images = flatland->CleanupFlatland2StateForTest({stack_content_handle});
  EXPECT_THAT(released_images, ::testing::ElementsAre(global_image_id));

  // The Flatland destructor will release all images.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, StackRemovalReleasesImageAtPresent) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties;
  properties.size(SizeU{100, 200});

  auto import_token_dup = ref_pair.DuplicateImportToken();
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto& global_collection_id = global_id_pair.collection_id;
  auto global_image_id = global_id_pair.image_id;

  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id);

  auto stack_content_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_content_handle, {layer});

  // Release the buffer collection.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
      .Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  import_token_dup.value().reset();
  RunLoopUntilIdle();

  // Release the classic image. Since it is bound to our live layer, it should NOT be released yet.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Set the stack to {}. Since the layer was only referenced by this stack, it is destroyed.
  flatland->SetLayerStackData(stack_content_handle, {});
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer), nullptr);

  // Call Present(); the image is queued for release behind that frame's release fence.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  // Signal the release fence: ReleaseBufferImage(id) is called once.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id), nullptr);
}

TEST_F(FlatlandTest, SharedImageReleasedWhenLastLayerDies) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();

  ImageProperties properties;
  properties.size(SizeU{100, 200});

  auto import_token_dup = ref_pair.DuplicateImportToken();
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto& global_collection_id = global_id_pair.collection_id;
  auto global_image_id = global_id_pair.image_id;

  // Release the buffer collection.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferCollection(global_collection_id, _))
      .Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  import_token_dup.value().reset();
  RunLoopUntilIdle();

  // Bind the same image to two test layers.
  LayerHandle layer1 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer1, global_image_id);
  LayerHandle layer2 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer2, global_image_id);

  auto* image_object = flatland->GetImageObjectForTest(global_image_id);
  ASSERT_NE(image_object, nullptr);
  // Facade layer has 1 ref, and layer1 and layer2 each hold 1 ref via SetLayerImageForTest.
  EXPECT_EQ(image_object->ref_count, 3u);

  // Put both layers in a stack.
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer1, layer2});

  // Release the facade content. The image remains live because layer1 and layer2 hold refs.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);
  EXPECT_EQ(image_object->ref_count, 2u);

  // Remove layer1 from the stack.
  flatland->SetLayerStackData(stack, {layer2});
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer1), nullptr);
  EXPECT_EQ(image_object->ref_count, 1u);

  // Present: image should not be released yet.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id), nullptr);

  // Remove layer2 from the stack.
  flatland->SetLayerStackData(stack, {});
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer2), nullptr);

  // Present: image queued for release, released once the release fence signals.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id), nullptr);
}

TEST_F(FlatlandTest, PerPresentReleaseFencesAreIndependent) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create image 1.
  const ContentId kImageId1(1);
  auto ref_pair1 = BufferCollectionImportExportTokens::New();
  ImageProperties properties1;
  properties1.size(SizeU{100, 200});
  const auto global_id_pair1 = CreateImage(flatland.get(), allocator.get(), kImageId1,
                                           std::move(ref_pair1), std::move(properties1));
  auto global_image_id1 = global_id_pair1.image_id;

  LayerHandle layer1 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer1, global_image_id1);
  auto stack1 = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack1, {layer1});
  flatland->ReleaseImage(kImageId1);
  Present(flatland, true);

  // Create image 2.
  const ContentId kImageId2(2);
  auto ref_pair2 = BufferCollectionImportExportTokens::New();
  ImageProperties properties2;
  properties2.size(SizeU{100, 200});
  const auto global_id_pair2 = CreateImage(flatland.get(), allocator.get(), kImageId2,
                                           std::move(ref_pair2), std::move(properties2));
  auto global_image_id2 = global_id_pair2.image_id;

  LayerHandle layer2 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer2, global_image_id2);
  auto stack2 = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack2, {layer2});
  flatland->ReleaseImage(kImageId2);
  Present(flatland, true);

  // Drop stack 1 -> layer 1 dies -> ref_count of image 1 drops to 0.
  flatland->SetLayerStackData(stack1, {});

  // Present 1: capture its release fence, but skip automatic signaling.
  fuchsia_ui_composition::wire::PresentArgs present_args1;
  flatland->Present(present_args1);

  std::vector<zx::event> fences1;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&fences1](zx::time, scheduling::SchedulingIdPair, bool,
                           std::vector<zx::event> release_fences, std::vector<zx::counter>,
                           std::vector<zx::counter>,
                           bool) { fences1 = std::move(release_fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});
  ASSERT_EQ(fences1.size(), 1u);
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);

  // Drop stack 2 -> layer 2 dies -> ref_count of image 2 drops to 0.
  flatland->SetLayerStackData(stack2, {});

  // Present 2: capture its release fence, but skip automatic signaling.
  fuchsia_ui_composition::wire::PresentArgs present_args2;
  flatland->Present(present_args2);

  std::vector<zx::event> fences2;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&fences2](zx::time, scheduling::SchedulingIdPair, bool,
                           std::vector<zx::event> release_fences, std::vector<zx::counter>,
                           std::vector<zx::counter>,
                           bool) { fences2 = std::move(release_fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});
  ASSERT_EQ(fences2.size(), 1u);
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 2u);

  // Signal the second fence and run the loop: only the second image is released, count is 1.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id1)).Times(0);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id2)).Times(1);
  fences2[0].signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);

  // Signal the first fence: the first image is released, count is 0.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id1)).Times(1);
  fences1[0].signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 0u);
}

TEST_F(FlatlandTest, PendingReleasesDrainedAtTeardown) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(SizeU{100, 200});

  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto global_image_id = global_id_pair.image_id;

  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id);
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer});
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Drop stack -> layer dies -> ref_count of image drops to 0.
  flatland->SetLayerStackData(stack, {});

  // Present, but skip signalling the release fence.
  PresentArgs args;
  args.skip_session_update_and_release_fences = true;
  PresentWithArgs(flatland, std::move(args), true);
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);

  // Destroy the session. Capture the RemoveSession release fence.
  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(0);
  flatland.reset();
  RunLoopUntilIdle();

  ASSERT_TRUE(teardown_fence.has_value());

  // Signal the teardown fence: the image is released.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, NoDoubleReleaseWhenPerPresentFenceSignalsAfterTeardown) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(SizeU{100, 200});

  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto global_image_id = global_id_pair.image_id;

  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id);
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer});
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Drop stack -> layer dies -> ref_count of image drops to 0.
  flatland->SetLayerStackData(stack, {});

  // 1. Present: capture that Present's release fence.
  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  std::vector<zx::event> per_present_fences;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&per_present_fences](zx::time, scheduling::SchedulingIdPair, bool,
                                      std::vector<zx::event> release_fences,
                                      std::vector<zx::counter>, std::vector<zx::counter>,
                                      bool) { per_present_fences = std::move(release_fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});
  ASSERT_EQ(per_present_fences.size(), 1u);
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);

  // 2. Destroy the session; capture the RemoveSession fence.
  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  flatland.reset();
  RunLoopUntilIdle();
  ASSERT_TRUE(teardown_fence.has_value());

  // 3. Signal the per-Present fence and run the loop: ReleaseBufferImage is NOT called.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(0);
  per_present_fences[0].signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();

  // 4. Signal the RemoveSession fence and run the loop: ReleaseBufferImage(id) exactly once.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, TeardownGathersRecordsAndAccumulator) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create image A.
  const ContentId kImageIdA(1);
  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  ImageProperties properties_a;
  properties_a.size(SizeU{100, 200});
  const auto global_id_pair_a = CreateImage(flatland.get(), allocator.get(), kImageIdA,
                                            std::move(ref_pair_a), std::move(properties_a));
  auto global_image_id_a = global_id_pair_a.image_id;

  LayerHandle layer_a = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer_a, global_image_id_a);
  auto stack_a = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_a, {layer_a});
  flatland->ReleaseImage(kImageIdA);
  Present(flatland, true);

  // Create image B.
  const ContentId kImageIdB(2);
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  ImageProperties properties_b;
  properties_b.size(SizeU{100, 200});
  const auto global_id_pair_b = CreateImage(flatland.get(), allocator.get(), kImageIdB,
                                            std::move(ref_pair_b), std::move(properties_b));
  auto global_image_id_b = global_id_pair_b.image_id;

  LayerHandle layer_b = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer_b, global_image_id_b);
  auto stack_b = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_b, {layer_b});
  flatland->ReleaseImage(kImageIdB);
  Present(flatland, true);

  // 1. Image A: drop last ref, Present.
  flatland->SetLayerStackData(stack_a, {});
  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_));
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);

  // 2. Image B: drop last ref, no Present (B stays in images_to_release_on_present_).
  flatland->SetLayerStackData(stack_b, {});

  // 3. Destroy session; capture RemoveSession fence.
  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_a)).Times(0);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_b)).Times(0);
  flatland.reset();
  RunLoopUntilIdle();

  ASSERT_TRUE(teardown_fence.has_value());

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_a)).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_b)).Times(1);
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

// Present() appends one internal fence to the client's release fences ONLY when there are one or
// more images to be released.  This test prevents a class of regressions where a fence is appended
// even though there were no images to be released.
TEST_F(FlatlandTest, ImageReleaseFenceAddedOnlyWhenImagesQueued) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create an image and keep it alive on a stack.
  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  ImageProperties properties;
  properties.size(SizeU{100, 200});
  const auto global_id_pair = CreateImage(flatland.get(), allocator.get(), kImageId,
                                          std::move(ref_pair), std::move(properties));
  auto global_image_id = global_id_pair.image_id;

  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id);
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer});
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  // Present with one client-supplied release fence and nothing to release:
  zx::event client_fence1 = utils::CreateEvent();
  std::vector<zx::event> client_fences1;
  client_fences1.push_back(utils::CopyZxHandle(client_fence1));

  fidl::Arena arena1;
  auto builder1 = fuchsia_ui_composition::wire::PresentArgs::Builder(arena1);
  builder1.release_fences(fidl::VectorView<zx::event>::FromExternal(client_fences1));
  auto present_args1 = builder1.Build();
  flatland->Present(present_args1);

  std::vector<zx::event> scheduled_fences1;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&scheduled_fences1](zx::time, scheduling::SchedulingIdPair, bool,
                                     std::vector<zx::event> release_fences,
                                     std::vector<zx::counter>, std::vector<zx::counter>,
                                     bool) { scheduled_fences1 = std::move(release_fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});

  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 0u);
  EXPECT_EQ(scheduled_fences1.size(), 1u);

  // Now drop the stack: image has 0 refs, will be released on next Present.
  flatland->SetLayerStackData(stack, {});

  zx::event client_fence2 = utils::CreateEvent();
  std::vector<zx::event> client_fences2;
  client_fences2.push_back(utils::CopyZxHandle(client_fence2));

  fidl::Arena arena2;
  auto builder2 = fuchsia_ui_composition::wire::PresentArgs::Builder(arena2);
  builder2.release_fences(fidl::VectorView<zx::event>::FromExternal(client_fences2));
  auto present_args2 = builder2.Build();
  flatland->Present(present_args2);

  std::vector<zx::event> scheduled_fences2;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&scheduled_fences2](zx::time, scheduling::SchedulingIdPair, bool,
                                     std::vector<zx::event> release_fences,
                                     std::vector<zx::counter>, std::vector<zx::counter>,
                                     bool) { scheduled_fences2 = std::move(release_fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});

  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);
  EXPECT_EQ(scheduled_fences2.size(), 2u);

  // Signal the image release fence to cleanly finish.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  scheduled_fences2[1].signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, OnePresentBatchesReleasesBehindOneFence) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Image 1.
  const ContentId kImageId1(1);
  auto ref_pair1 = BufferCollectionImportExportTokens::New();
  ImageProperties properties1;
  properties1.size(SizeU{100, 200});
  const auto global_id_pair1 = CreateImage(flatland.get(), allocator.get(), kImageId1,
                                           std::move(ref_pair1), std::move(properties1));
  auto global_image_id1 = global_id_pair1.image_id;

  LayerHandle layer1 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer1, global_image_id1);
  auto stack1 = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack1, {layer1});
  flatland->ReleaseImage(kImageId1);
  Present(flatland, true);

  // Image 2.
  const ContentId kImageId2(2);
  auto ref_pair2 = BufferCollectionImportExportTokens::New();
  ImageProperties properties2;
  properties2.size(SizeU{100, 200});
  const auto global_id_pair2 = CreateImage(flatland.get(), allocator.get(), kImageId2,
                                           std::move(ref_pair2), std::move(properties2));
  auto global_image_id2 = global_id_pair2.image_id;

  LayerHandle layer2 = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer2, global_image_id2);
  auto stack2 = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack2, {layer2});
  flatland->ReleaseImage(kImageId2);
  Present(flatland, true);

  // Drop last refs of both images before presenting.
  flatland->SetLayerStackData(stack1, {});
  flatland->SetLayerStackData(stack2, {});

  fuchsia_ui_composition::wire::PresentArgs present_args;
  flatland->Present(present_args);

  std::vector<zx::event> release_fences;
  EXPECT_CALL(*mock_flatland_presenter_,
              ScheduleUpdateForSession(::testing::_, ::testing::_, ::testing::_, ::testing::_,
                                       ::testing::_, ::testing::_, ::testing::_))
      .WillOnce([&release_fences](zx::time, scheduling::SchedulingIdPair, bool,
                                  std::vector<zx::event> fences, std::vector<zx::counter>,
                                  std::vector<zx::counter>,
                                  bool) { release_fences = std::move(fences); });
  RunLoopUntilIdle();
  flatland->OnNextFrameBegin(1, {});

  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 1u);
  ASSERT_EQ(release_fences.size(), 1u);

  // Signal the single fence: both images are released, count is 0.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id1)).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id2)).Times(1);
  release_fences[0].signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 0u);
}

// The "test seam" is `SetLayerImageForTest()`, the stand-in for the not-yet-implemented Flatland2
// `SetLayerImage` FIDL method.  Until then, there is no way to trigger `BindLayerImage()` on a
// layer that already has a binding.
TEST_F(FlatlandTest, RebindViaTestSeamReleasesOldImage) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create image A.
  const ContentId kImageIdA(1);
  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  ImageProperties properties_a;
  properties_a.size(SizeU{100, 200});
  const auto global_id_pair_a = CreateImage(flatland.get(), allocator.get(), kImageIdA,
                                            std::move(ref_pair_a), std::move(properties_a));
  auto global_image_id_a = global_id_pair_a.image_id;

  // Create image B.
  const ContentId kImageIdB(2);
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  ImageProperties properties_b;
  properties_b.size(SizeU{100, 200});
  const auto global_id_pair_b = CreateImage(flatland.get(), allocator.get(), kImageIdB,
                                            std::move(ref_pair_b), std::move(properties_b));
  auto global_image_id_b = global_id_pair_b.image_id;

  // Bind A to a test layer and B to a holder layer so they survive facade release.
  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id_a);

  LayerHandle layer_b = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer_b, global_image_id_b);
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer, layer_b});

  // Release their facade contents so only seam refs remain.
  flatland->ReleaseImage(kImageIdA);
  flatland->ReleaseImage(kImageIdB);
  Present(flatland, true);

  // Image A has exactly 1 ref (layer), image B has exactly 1 ref (layer_b).
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id_a), nullptr);
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id_b), nullptr);

  // Now rebind to image B on the same layer.
  flatland->SetLayerImageForTest(layer, global_image_id_b);
  // Image A's ref count dropped to 0 and its ImageObject was destroyed.
  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id_a), nullptr);
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id_b), nullptr);

  // Present: image A queued for release.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  // Signal fence: A released once, B not released.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_a)).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_b)).Times(0);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id_a), nullptr);
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id_b), nullptr);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_b)).Times(1);
}

// The "test seam" is `SetLayerImageForTest()`, the stand-in for the not-yet-implemented Flatland2
// `SetLayerImage` FIDL method.  Until then, there is no way to trigger `BindLayerImage()` on a
// layer that already has a binding.
TEST_F(FlatlandTest, RebindSameImageViaTestSeamIsNoOp) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  // Create image A.
  const ContentId kImageIdA(1);
  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  ImageProperties properties_a;
  properties_a.size(SizeU{100, 200});
  const auto global_id_pair_a = CreateImage(flatland.get(), allocator.get(), kImageIdA,
                                            std::move(ref_pair_a), std::move(properties_a));
  auto global_image_id_a = global_id_pair_a.image_id;

  // Bind A to a test layer via the seam.
  LayerHandle layer = flatland->CreateLayerObject();
  flatland->SetLayerImageForTest(layer, global_image_id_a);
  auto stack = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack, {layer});

  // Release the facade content so only the seam ref remains.
  flatland->ReleaseImage(kImageIdA);
  Present(flatland, true);

  // Image A has exactly 1 ref (the seam layer).
  auto* image_object = flatland->GetImageObjectForTest(global_image_id_a);
  ASSERT_NE(image_object, nullptr);
  EXPECT_EQ(image_object->ref_count, 1u);

  // Rebind the same image to the layer.
  flatland->SetLayerImageForTest(layer, global_image_id_a);
  image_object = flatland->GetImageObjectForTest(global_image_id_a);
  ASSERT_NE(image_object, nullptr);
  EXPECT_EQ(image_object->ref_count, 1u);
  auto* layer_object = flatland->GetLayerObjectForTest(layer);
  ASSERT_NE(layer_object, nullptr);
  EXPECT_EQ(layer_object->image_mode.image_id, global_image_id_a);

  // Present, then signal the fence and run the loop: ReleaseBufferImage Times(0).
  // PendingImageReleaseCountForTest() == 0 after the Present (nothing was queued).
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);
  EXPECT_EQ(flatland->PendingImageReleaseCountForTest(), 0u);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id_a)).Times(1);
}

TEST_F(FlatlandTest, CreateImageImportFailureRollsBackImageObject) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::optional<std::string> error_log;
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator.get(), std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{150, 175})
                        .Build();

  allocation::GlobalImageId failed_image_id = allocation::kInvalidImageId;
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce([&failed_image_id](const allocation::ImageMetadata& metadata,
                                   allocation::BufferCollectionUsage) {
        failed_image_id = metadata.identifier;
        return fpromise::make_error_promise();
      });

  flatland->CreateImage(kImageId, ToWire(ref_pair.DuplicateImportToken()), /*vmo_idx*/ 0,
                        properties);
  RunLoopUntilIdle();

  ASSERT_TRUE(error_log.has_value());
  ASSERT_NE(failed_image_id, allocation::kInvalidImageId);

  // The import failure closed the FIDL connection, but the fixture's destroy-instance callback
  // is a no-op and this test still holds a shared_ptr, so `flatland` is alive and inspectable.
  // It is destroyed explicitly in step 3.

  // 1. GetImageObjectForTest(id) is nullptr.
  EXPECT_EQ(flatland->GetImageObjectForTest(failed_image_id), nullptr);

  // 2. The facade layer's binding is cleared.
  auto content_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(content_handle.has_value());
  auto* stack_data = flatland->GetLayerStackDataForTest(*content_handle);
  ASSERT_NE(stack_data, nullptr);
  ASSERT_FALSE(stack_data->layers.empty());
  auto* layer_obj = flatland->GetLayerObjectForTest(stack_data->layers[0]);
  ASSERT_NE(layer_obj, nullptr);
  EXPECT_EQ(layer_obj->image_mode.image_id, allocation::kInvalidImageId);

  // 3. Destroying the session releases the image exactly once, through the RemoveSession
  // fence: the binding was dropped through the ref count, so the id rides the normal path
  // and the importer ignores it.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(failed_image_id)).Times(1);

  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  flatland.reset();
  RunLoopUntilIdle();

  ASSERT_TRUE(teardown_fence.has_value());
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

// The first image's import fails only after the client has released it and reused its content
// id for a second image.  The rollback must find the first image's layer by its handle and leave
// the second image alone.
TEST_F(FlatlandTest, LateImportFailureLeavesReusedContentIdIntact) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::optional<std::string> error_log;
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));

  const ContentId kImageId(1);
  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator.get(), std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{150, 175})
                        .Build();

  fpromise::bridge<> first_import;
  allocation::GlobalImageId first_id = allocation::kInvalidImageId;
  allocation::GlobalImageId second_id = allocation::kInvalidImageId;
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce([&](const allocation::ImageMetadata& metadata, allocation::BufferCollectionUsage) {
        first_id = metadata.identifier;
        return first_import.consumer.promise();
      })
      .WillOnce([&](const allocation::ImageMetadata& metadata, allocation::BufferCollectionUsage) {
        second_id = metadata.identifier;
        return fpromise::make_ok_promise();
      });

  flatland->CreateImage(kImageId, ToWire(ref_pair.DuplicateImportToken()), /*vmo_idx*/ 0,
                        properties);
  RunLoopUntilIdle();
  EXPECT_FALSE(error_log.has_value());

  flatland->ReleaseImage(kImageId);
  flatland->CreateImage(kImageId, ToWire(ref_pair.DuplicateImportToken()), /*vmo_idx*/ 0,
                        properties);
  RunLoopUntilIdle();

  first_import.completer.complete_error();
  RunLoopUntilIdle();
  ASSERT_TRUE(error_log.has_value());

  // Assertions after the failure:
  // - GetImageObjectForTest(first_id) is null (the released layer was still
  //   alive, so its binding was dropped and the object died with its last ref).
  EXPECT_EQ(flatland->GetImageObjectForTest(first_id), nullptr);

  // - GetImageObjectForTest(second_id) is non-null with ref_count == 1u.
  auto* image_obj_2 = flatland->GetImageObjectForTest(second_id);
  ASSERT_NE(image_obj_2, nullptr);
  EXPECT_EQ(image_obj_2->ref_count, 1u);

  // - The layer under GetContentHandle(kImageId) (stack data, layers[0])
  //   has image_mode.image_id == second_id.
  auto content_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(content_handle.has_value());
  auto* stack_data = flatland->GetLayerStackDataForTest(*content_handle);
  ASSERT_NE(stack_data, nullptr);
  ASSERT_FALSE(stack_data->layers.empty());
  auto* layer_obj = flatland->GetLayerObjectForTest(stack_data->layers[0]);
  ASSERT_NE(layer_obj, nullptr);
  EXPECT_EQ(layer_obj->image_mode.image_id, second_id);

  // Teardown: capture the RemoveSession fence, flatland.reset(),
  // RunLoopUntilIdle(), expect ReleaseBufferImage(first_id) once and
  // ReleaseBufferImage(second_id) once, signal the fence, RunLoopUntilIdle().
  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  flatland.reset();
  RunLoopUntilIdle();

  ASSERT_TRUE(teardown_fence.has_value());
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(first_id)).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(second_id)).Times(1);
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

TEST_F(FlatlandTest, EpochIncrementsOnTypeTransition) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();

  LayerHandle layer = flatland->CreateLayerObject();
  const auto* obj = flatland->GetLayerObjectForTest(layer);
  ASSERT_NE(obj, nullptr);
  EXPECT_TRUE(obj->mode == LayerObject::Mode::kInvisible);

  // Transition to Image.
  flatland->SetLayerImageForTest(layer, allocation::kInvalidImageId);
  EXPECT_TRUE(obj->mode == LayerObject::Mode::kImage);

  // Transition to same kind: epoch should not increment.
  flatland->SetLayerImageForTest(layer, allocation::kInvalidImageId);

  // Transition to SolidColor.
  flatland->SetLayerSolidColorForTest(layer);
  EXPECT_TRUE(obj->mode == LayerObject::Mode::kSolidColor);
}

TEST_F(FlatlandTest, PresentStampsFlatlandVersion) {
  // Classic session Present -> snapshot flatland_version == 1
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    Present(flatland, true);
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_EQ(uber_struct->flatland_version, 1u);
  }

  // Flatland2 session -> snapshot flatland_version == 2
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland(FlatlandConfig{.use_flatland2 = true});
    Present(flatland, true);
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_EQ(uber_struct->flatland_version, 2u);
  }
}

TEST_F(Flatland2Test, EmptyStackHasNoRefCountEffect) {
  std::shared_ptr<Flatland> flatland = CreateFlatland2();
  LayerHandle layer_a = flatland->CreateLayerObject();
  LayerHandle layer_b = flatland->CreateLayerObject();

  auto stack_handle = flatland->CreateLayerStackData();

  const auto* obj_a = flatland->GetLayerObjectForTest(layer_a);
  const auto* obj_b = flatland->GetLayerObjectForTest(layer_b);
  ASSERT_NE(obj_a, nullptr);
  ASSERT_NE(obj_b, nullptr);
  EXPECT_EQ(obj_a->ref_count, 0);
  EXPECT_EQ(obj_b->ref_count, 0);
}

TEST_F(Flatland2Test, SetLayersAddRemoveUpdatesRefCounts) {
  std::shared_ptr<Flatland> flatland = CreateFlatland2();
  LayerHandle layer_a = flatland->CreateLayerObject();
  LayerHandle layer_b = flatland->CreateLayerObject();
  LayerHandle layer_c = flatland->CreateLayerObject();

  auto stack_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_handle, {layer_a, layer_b});

  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_a)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_b)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_c)->ref_count, 0);

  // Now set stack to {B, C}. A's count decrements (hit 0 -> destroyed),
  // B unchanged, C incremented.
  flatland->SetLayerStackData(stack_handle, {layer_b, layer_c});

  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_a), nullptr);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_b)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_c)->ref_count, 1);
}

TEST_F(Flatland2Test, ReorderIsRefCountNeutral) {
  std::shared_ptr<Flatland> flatland = CreateFlatland2();
  LayerHandle layer_a = flatland->CreateLayerObject();
  LayerHandle layer_b = flatland->CreateLayerObject();

  auto stack_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_handle, {layer_a, layer_b});

  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_a)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_b)->ref_count, 1);

  flatland->SetLayerStackData(stack_handle, {layer_b, layer_a});

  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_a)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_b)->ref_count, 1);

  const auto* stack_data = flatland->GetLayerStackDataForTest(stack_handle);
  ASSERT_NE(stack_data, nullptr);
  EXPECT_THAT(stack_data->layers, ::testing::ElementsAre(layer_b, layer_a));
}

TEST_F(Flatland2Test, ReleaseLayerReleasesBoundImage) {
  auto flatland = CreateFlatland2();
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  LayerHandle handle = flatland->GetLayerHandleForTest(kId);
  ASSERT_NE(handle, LayerHandle());

  const allocation::GlobalImageId image_id = allocation::GenerateUniqueImageId();
  flatland->SetLayerImageForTest(handle, image_id);
  EXPECT_NE(flatland->GetImageObjectForTest(image_id), nullptr);

  flatland->ReleaseLayer(kId);

  // Present: image queued for release, released once the release fence signals.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(image_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_EQ(flatland->GetImageObjectForTest(image_id), nullptr);
}

TEST_F(Flatland2Test, ClearReleasesClientHeldLayers) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  LayerHandle handle = flatland->GetLayerHandleForTest(kId);
  ASSERT_NE(handle, LayerHandle());

  const allocation::GlobalImageId image_id = allocation::GenerateUniqueImageId();
  flatland->SetLayerImageForTest(handle, image_id);
  EXPECT_NE(flatland->GetImageObjectForTest(image_id), nullptr);

  flatland->Clear();

  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(image_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  EXPECT_EQ(flatland->GetImageObjectForTest(image_id), nullptr);
}

TEST_F(Flatland2Test, TeardownReleasesClientHeldLayerImage) {
  auto flatland = CreateFlatland2();
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  LayerHandle handle = flatland->GetLayerHandleForTest(kId);
  ASSERT_NE(handle, LayerHandle());

  const allocation::GlobalImageId image_id = allocation::GenerateUniqueImageId();
  flatland->SetLayerImageForTest(handle, image_id);
  EXPECT_NE(flatland->GetImageObjectForTest(image_id), nullptr);

  std::optional<zx::event> teardown_fence;
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(flatland->GetSessionId(), ::testing::_))
      .WillOnce([&teardown_fence](scheduling::SessionId, std::optional<zx::event> fence) {
        teardown_fence = std::move(fence);
      });

  flatland.reset();
  RunLoopUntilIdle();

  ASSERT_TRUE(teardown_fence.has_value());
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(image_id)).Times(1);
  teardown_fence->signal(0, ZX_EVENT_SIGNALED);
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, SetLayersOnDeadStackResultsInError) {
  std::shared_ptr<Flatland> flatland = CreateFlatland2();
  LayerHandle layer = flatland->CreateLayerObject();
  TransformHandle dead_stack_handle = {999, 999};
  EXPECT_DEATH(flatland->SetLayerStackData(dead_stack_handle, {layer}), "");
}

TEST_F(Flatland2Test, CreateReleaseRecreateSameId) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  flatland->ReleaseLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());
}

TEST_F(Flatland2Test, CreateLayerDuplicateIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  flatland->CreateLayer(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, CreateLayerZeroIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kZeroId(0);
  flatland->CreateLayer(kZeroId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerUnknownIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->ReleaseLayer(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerDoubleReleaseFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  flatland->ReleaseLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  flatland->ReleaseLayer(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ClientRefKeepsLayerAlive) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  LayerHandle handle = flatland->GetLayerHandleForTest(kId);
  ASSERT_NE(handle, LayerHandle());

  LayerObject* obj = flatland->GetLayerObjectForTest(handle);
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->ref_count, 1);

  // Verify LayerObject survives session garbage collection.
  Present(flatland, true);

  obj = flatland->GetLayerObjectForTest(handle);
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->ref_count, 1);

  flatland->ReleaseLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  obj = flatland->GetLayerObjectForTest(handle);
  EXPECT_EQ(obj, nullptr);
}

TEST_F(Flatland2Test, LayerSurvivesReleaseWhileStacked) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  LayerHandle handle = flatland->GetLayerHandleForTest(kId);
  ASSERT_NE(handle, LayerHandle());

  TransformHandle stack_handle = flatland->CreateLayerStackData();
  flatland->SetLayerStackData(stack_handle, {handle});

  LayerObject* obj = flatland->GetLayerObjectForTest(handle);
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->ref_count, 2);

  flatland->ReleaseLayer(kId);
  EXPECT_FALSE(error_log.has_value());

  // Object should still be alive because the stack holds a reference.
  obj = flatland->GetLayerObjectForTest(handle);
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->ref_count, 1);

  // Removing from the stack drops the last ref and destroys the object.
  flatland->SetLayerStackData(stack_handle, {});
  obj = flatland->GetLayerObjectForTest(handle);
  EXPECT_EQ(obj, nullptr);
}

TEST_F(Flatland2Test, CreateLayerFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  const LayerId kId(1);
  flatland->CreateLayer(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  const LayerId kId(1);
  flatland->ReleaseLayer(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, CreateLayerStackZeroIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->CreateLayerStack(LayerStackId(0));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, CreateLayerStackDuplicateIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kId(1);
  flatland->CreateLayerStack(kId);
  EXPECT_FALSE(error_log.has_value());

  flatland->CreateLayerStack(kId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, CreateLayerAndStackSameIdSucceeds) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->CreateLayer(LayerId(7));
  EXPECT_FALSE(error_log.has_value());

  flatland->CreateLayerStack(LayerStackId(7));
  EXPECT_FALSE(error_log.has_value());
}

TEST_F(Flatland2Test, CreateLayerStackFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  flatland->CreateLayerStack(LayerStackId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerStackZeroIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->ReleaseLayerStack(LayerStackId(0));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerStackUnknownIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->ReleaseLayerStack(LayerStackId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerStackUnknownIdEvenIfLayerExists) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->CreateLayer(LayerId(1));
  EXPECT_FALSE(error_log.has_value());

  flatland->ReleaseLayerStack(LayerStackId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ReleaseLayerStackFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  flatland->ReleaseLayerStack(LayerStackId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersZeroStackIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->SetStackLayers(LayerStackId(0), {});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersUnknownStackIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->SetStackLayers(LayerStackId(1), {});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersUnknownStackIdEvenIfLayerExists) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->CreateLayer(LayerId(1));
  EXPECT_FALSE(error_log.has_value());

  flatland->SetStackLayers(LayerStackId(1), {});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersZeroLayerIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId(1);
  flatland->CreateLayerStack(kStackId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetStackLayers(kStackId, {LayerId(0)});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersUnknownLayerIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId(1);
  flatland->CreateLayerStack(kStackId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetStackLayers(kStackId, {LayerId(2)});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersUnknownLayerIdEvenIfStackExists) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId1(1);
  const LayerStackId kStackId2(2);
  flatland->CreateLayerStack(kStackId1);
  flatland->CreateLayerStack(kStackId2);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetStackLayers(kStackId1, {LayerId(2)});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersDuplicateLayerIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId(1);
  const LayerId kLayerId(2);
  flatland->CreateLayerStack(kStackId);
  flatland->CreateLayer(kLayerId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetStackLayers(kStackId, {kLayerId, kLayerId});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  flatland->SetStackLayers(LayerStackId(1), {});
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetStackLayersAdjustsRefCounts) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayerA(1);
  const LayerId kLayerB(2);
  const LayerId kLayerC(3);
  const LayerStackId kStack(10);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayer(kLayerC);
  flatland->CreateLayerStack(kStack);

  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});
  EXPECT_FALSE(error_log.has_value());

  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);
  LayerHandle handle_b = flatland->GetLayerHandleForTest(kLayerB);
  LayerHandle handle_c = flatland->GetLayerHandleForTest(kLayerC);

  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_c)->ref_count, 1);

  flatland->SetStackLayers(kStack, {kLayerB, kLayerC});
  EXPECT_FALSE(error_log.has_value());

  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_c)->ref_count, 2);
}

TEST_F(Flatland2Test, ReleasedLayerKeptAliveByStack) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayerA(1);
  const LayerStackId kStack(10);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA});

  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);
  EXPECT_NE(flatland->GetLayerObjectForTest(handle_a), nullptr);

  flatland->ReleaseLayer(kLayerA);
  EXPECT_FALSE(error_log.has_value());

  // Object still alive because stack holds reference.
  EXPECT_NE(flatland->GetLayerObjectForTest(handle_a), nullptr);

  flatland->SetStackLayers(kStack, {});
  EXPECT_FALSE(error_log.has_value());

  // Destroyed after stack reference dropped.
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a), nullptr);
}

TEST_F(Flatland2Test, ReleaseStackWithLayers) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayerA(1);
  const LayerId kLayerB(2);
  const LayerStackId kStack(10);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});

  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);
  LayerHandle handle_b = flatland->GetLayerHandleForTest(kLayerB);

  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 2);

  flatland->ReleaseLayerStack(kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  // Stack handle dead-transform GC freed the stack's layer references, so client refs remain.
  EXPECT_NE(flatland->GetLayerObjectForTest(handle_a), nullptr);
  EXPECT_NE(flatland->GetLayerObjectForTest(handle_b), nullptr);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 1);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 1);
}

TEST_F(Flatland2Test, NoPartialApplicationOnBadLayerId) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayerA(1);
  const LayerId kLayerB(2);
  const LayerId kBogusLayer(999);
  const LayerStackId kStack(10);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA});

  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);
  LayerHandle handle_b = flatland->GetLayerHandleForTest(kLayerB);
  auto stack_handle = flatland->GetLayerStackHandleForTest(kStack);
  ASSERT_TRUE(stack_handle.has_value());

  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 1);

  flatland->SetStackLayers(kStack, {kLayerA, kBogusLayer, kLayerB});
  ASSERT_TRUE(error_log.has_value());

  // Stack contents and ref counts remain unchanged.
  const auto* stack_data = flatland->GetLayerStackDataForTest(*stack_handle);
  ASSERT_NE(stack_data, nullptr);
  EXPECT_THAT(stack_data->layers, ::testing::ElementsAre(handle_a));

  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a)->ref_count, 2);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_b)->ref_count, 1);
  Present(flatland, false);
}

TEST_F(Flatland2Test, UnattachedStackNotSnapshotted) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayerId(1);
  const LayerStackId kStackId(2);

  flatland->CreateLayer(kLayerId);
  flatland->CreateLayerStack(kStackId);
  flatland->SetStackLayers(kStackId, {kLayerId});

  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);
  EXPECT_TRUE(uber_struct->layer_stacks.empty());
}

TEST_F(Flatland2Test, AttachedStackAppearsInSnapshot) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerId kLayerB(3);
  const LayerStackId kStack(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});

  flatland->SetTransformContent(kRoot, kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);

  auto stack_handle = flatland->GetLayerStackHandleForTest(kStack);
  ASSERT_TRUE(stack_handle.has_value());

  ASSERT_TRUE(uber_struct->layer_stacks.contains(*stack_handle));
  const auto& stack_layers = uber_struct->layer_stacks.find(*stack_handle)->second;

  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);
  LayerHandle handle_b = flatland->GetLayerHandleForTest(kLayerB);
  EXPECT_THAT(stack_layers, ::testing::ElementsAre(handle_a, handle_b));

  ASSERT_EQ(uber_struct->layers.size(), 2u);
  ASSERT_TRUE(uber_struct->layers.contains(handle_a));
  ASSERT_TRUE(uber_struct->layers.contains(handle_b));
  EXPECT_TRUE(
      std::holds_alternative<std::monostate>(uber_struct->layers.find(handle_a)->second.content));
  EXPECT_TRUE(
      std::holds_alternative<std::monostate>(uber_struct->layers.find(handle_b)->second.content));
}

TEST_F(Flatland2Test, MonostateLayersProduceNoRenderables) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerId kLayerB(3);
  const LayerStackId kStack(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});

  flatland->SetTransformContent(kRoot, kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  EXPECT_TRUE(resolved_layers.empty());
}

TEST_F(Flatland2Test, DetachRemovesFromSnapshot) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerStackId kStack(3);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA});

  flatland->SetTransformContent(kRoot, kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  auto stack_handle = flatland->GetLayerStackHandleForTest(kStack);
  ASSERT_TRUE(stack_handle.has_value());
  LayerHandle handle_a = flatland->GetLayerHandleForTest(kLayerA);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_handle));
    EXPECT_TRUE(uber_struct->layers.contains(handle_a));
  }

  // Release the stack and layer from client while attached.
  flatland->ReleaseLayer(kLayerA);
  flatland->ReleaseLayerStack(kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  // Still snapshotted and alive because attached to transform.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_handle));
    EXPECT_TRUE(uber_struct->layers.contains(handle_a));
  }

  // Detach content (omit content).
  flatland->SetTransformContent(kRoot, nullptr);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  // Stack is now gone from snapshot, and layer object is GC'd.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_FALSE(uber_struct->layer_stacks.contains(*stack_handle));
    EXPECT_FALSE(uber_struct->layers.contains(handle_a));
  }
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle_a), nullptr);
}

TEST_F(Flatland2Test, MultiAttachSameStack) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const TransformId kChild1(2);
  const TransformId kChild2(3);
  const LayerId kLayerA(4);
  const LayerStackId kStack(5);

  flatland->CreateTransform(kRoot);
  flatland->CreateTransform(kChild1);
  flatland->CreateTransform(kChild2);
  flatland->AddChild(kRoot, kChild1);
  flatland->AddChild(kRoot, kChild2);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA});

  // Attach the same stack to two transforms.
  flatland->SetTransformContent(kChild1, kStack);
  flatland->SetTransformContent(kChild2, kStack);
  EXPECT_FALSE(error_log.has_value());

  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);

  auto stack_handle = flatland->GetLayerStackHandleForTest(kStack);
  ASSERT_TRUE(stack_handle.has_value());

  // Snapshot has exactly one layer_stacks entry and one layer.
  EXPECT_EQ(uber_struct->layer_stacks.size(), 1u);
  EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_handle));
  EXPECT_EQ(uber_struct->layers.size(), 1u);

  // Topology contains the stack handle twice (under child1 and child2).
  TransformHandle child1_handle = flatland->GetTransformHandle(kChild1).value();
  TransformHandle child2_handle = flatland->GetTransformHandle(kChild2).value();
  ASSERT_EQ(uber_struct->local_topology.size(), 6u);
  EXPECT_EQ(uber_struct->local_topology[2].handle, child1_handle);
  EXPECT_EQ(uber_struct->local_topology[2].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[3].handle, *stack_handle);
  EXPECT_EQ(uber_struct->local_topology[3].child_count, 0u);
  EXPECT_EQ(uber_struct->local_topology[4].handle, child2_handle);
  EXPECT_EQ(uber_struct->local_topology[4].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[5].handle, *stack_handle);
  EXPECT_EQ(uber_struct->local_topology[5].child_count, 0u);
}

TEST_F(Flatland2Test, SwapStacks) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerId kLayerB(3);
  const LayerStackId kStackA(4);
  const LayerStackId kStackB(5);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStackA);
  flatland->CreateLayerStack(kStackB);
  flatland->SetStackLayers(kStackA, {kLayerA});
  flatland->SetStackLayers(kStackB, {kLayerB});

  auto stack_a_handle = flatland->GetLayerStackHandleForTest(kStackA);
  auto stack_b_handle = flatland->GetLayerStackHandleForTest(kStackB);
  ASSERT_TRUE(stack_a_handle.has_value());
  ASSERT_TRUE(stack_b_handle.has_value());

  // Attach Stack A
  flatland->SetTransformContent(kRoot, kStackA);
  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_a_handle));
    EXPECT_FALSE(uber_struct->layer_stacks.contains(*stack_b_handle));
  }

  // Swap to Stack B
  flatland->SetTransformContent(kRoot, kStackB);
  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_FALSE(uber_struct->layer_stacks.contains(*stack_a_handle));
    EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_b_handle));
  }

  // Swap back to Stack A
  flatland->SetTransformContent(kRoot, kStackA);
  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    EXPECT_TRUE(uber_struct->layer_stacks.contains(*stack_a_handle));
    EXPECT_FALSE(uber_struct->layer_stacks.contains(*stack_b_handle));
  }
}

TEST_F(Flatland2Test, SetTransformContentZeroTransformFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId(1);
  flatland->CreateLayerStack(kStackId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(TransformId(0), kStackId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentUnknownTransformFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerStackId kStackId(1);
  flatland->CreateLayerStack(kStackId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(TransformId(2), kStackId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentUnknownStackIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kTransformId(1);
  flatland->CreateTransform(kTransformId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(kTransformId, LayerStackId(2));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentZeroLayerStackIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kTransformId(1);
  flatland->CreateTransform(kTransformId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(kTransformId, LayerStackId(0));
  ASSERT_TRUE(error_log.has_value());
  EXPECT_EQ(
      *error_log,
      "SetTransformContent: LayerStackId must be non-zero (to clear content, omit `content`)");
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentZeroViewportIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kTransformId(1);
  flatland->CreateTransform(kTransformId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(kTransformId, ViewportId(0));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentUnknownStackIdEvenIfLayerExists) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kTransformId(1);
  const LayerId kLayerId(2);
  flatland->CreateTransform(kTransformId);
  flatland->CreateLayer(kLayerId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(kTransformId, LayerStackId(2));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentViewportArmNotImplementedFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kTransformId(1);
  flatland->CreateTransform(kTransformId);
  EXPECT_FALSE(error_log.has_value());

  flatland->SetTransformContent(kTransformId, ViewportId(2));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetTransformContentFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  // Create valid transform and layer stack to ensure SetTransformContent doesn't
  // fail from other errors.
  const TransformId kTransformId(1);
  const LayerStackId kStackId(2);
  flatland->CreateTransform(kTransformId);
  flatland->CreateLayerStack(kStackId);
  flatland->SetTransformContent(kTransformId, kStackId);
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, PropertiesStickyAcrossModeChanges) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  // Set properties across all groups.
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{10, 20, 100, 200});
  props.opacity(0.75f);
  props.blend_mode(fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha);
  props.color(fuchsia_ui_composition::ColorRgba{0.2f, 0.4f, 0.6f, 0.8f});
  props.sample_rect(fuchsia_math::RectF{5.f, 10.f, 50.f, 100.f});
  props.transform(fuchsia_ui_composition::FlipThenRotate::kFlipH |
                  fuchsia_ui_composition::FlipThenRotate::kRotate90);
  props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // 1. IMAGE mode: common + image_mode active.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.display_rect,
              (types::Rectangle({.x = 10, .y = 20, .width = 100, .height = 200})));
    EXPECT_EQ(us_layer.common.opacity, 0.75f);
    EXPECT_EQ(us_layer.common.blend_mode, types::BlendMode::kPremultipliedAlpha());
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
    const auto& img = std::get<UberStructLayer::ImageModeProperties>(us_layer.content);
    EXPECT_EQ(img.sample_rect,
              (types::RectangleF({.x = 5.f, .y = 10.f, .width = 50.f, .height = 100.f})));
    EXPECT_EQ(img.transform, types::RotateFlip::kRotateCcw90ReflectY());
  }

  // 2. Switch to SOLID_COLOR mode.
  fuchsia_ui_composition::LayerProperties solid_mode_props;
  solid_mode_props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(solid_mode_props)));
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.display_rect,
              (types::Rectangle({.x = 10, .y = 20, .width = 100, .height = 200})));
    EXPECT_EQ(us_layer.common.opacity, 0.75f);
    EXPECT_EQ(us_layer.common.blend_mode, types::BlendMode::kPremultipliedAlpha());
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    const auto& solid = std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content);
    EXPECT_EQ(solid.color, (std::array<float, 4>{0.2f, 0.4f, 0.6f, 0.8f}));
  }

  // 3. Switch back to IMAGE mode (second leg of double round trip).
  fuchsia_ui_composition::LayerProperties image_mode_props;
  image_mode_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(image_mode_props)));
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
    const auto& img = std::get<UberStructLayer::ImageModeProperties>(us_layer.content);
    EXPECT_EQ(img.sample_rect,
              (types::RectangleF({.x = 5.f, .y = 10.f, .width = 50.f, .height = 100.f})));
    EXPECT_EQ(img.transform, types::RotateFlip::kRotateCcw90ReflectY());
  }

  // 4. Switch back to SOLID_COLOR mode again.
  fuchsia_ui_composition::LayerProperties solid_mode_props2;
  solid_mode_props2.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(solid_mode_props2)));
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    const auto& solid = std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content);
    EXPECT_EQ(solid.color, (std::array<float, 4>{0.2f, 0.4f, 0.6f, 0.8f}));
  }
}

TEST_F(Flatland2Test, MergeAccumulatesSparseTables) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  // Set mode to IMAGE.
  {
    fuchsia_ui_composition::LayerProperties props;
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 1: display_rect
  {
    fuchsia_ui_composition::LayerProperties props;
    props.display_rect(fuchsia_math::RectU{0, 0, 80, 80});
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 2: opacity
  {
    fuchsia_ui_composition::LayerProperties props;
    props.opacity(0.5f);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 3: blend_mode
  {
    fuchsia_ui_composition::LayerProperties props;
    props.blend_mode(fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 4: sample_rect
  {
    fuchsia_ui_composition::LayerProperties props;
    props.sample_rect(fuchsia_math::RectF{0.f, 0.f, 40.f, 40.f});
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 5: transform
  {
    fuchsia_ui_composition::LayerProperties props;
    props.transform(fuchsia_ui_composition::FlipThenRotate::kFlipV);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  // Sparse call 6: color (authored while in IMAGE mode!)
  {
    fuchsia_ui_composition::LayerProperties props;
    props.color(fuchsia_ui_composition::ColorRgba{0.1f, 0.2f, 0.3f, 0.4f});
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // In IMAGE mode: all accumulated properties for common and image_mode appear.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.display_rect,
              (types::Rectangle({.x = 0, .y = 0, .width = 80, .height = 80})));
    EXPECT_EQ(us_layer.common.opacity, 0.5f);
    EXPECT_EQ(us_layer.common.blend_mode, types::BlendMode::kPremultipliedAlpha());
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
    const auto& img = std::get<UberStructLayer::ImageModeProperties>(us_layer.content);
    EXPECT_EQ(img.sample_rect,
              (types::RectangleF({.x = 0.f, .y = 0.f, .width = 40.f, .height = 40.f})));
    EXPECT_EQ(img.transform, types::RotateFlip::kReflectX());
  }

  // Switch to SOLID_COLOR mode: previously-authored color takes effect!
  {
    fuchsia_ui_composition::LayerProperties props;
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    const auto& solid = std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content);
    EXPECT_EQ(solid.color, (std::array<float, 4>{0.1f, 0.2f, 0.3f, 0.4f}));
  }
}

TEST_F(Flatland2Test, ModeIsStickyAndMergeable) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // Set initial state with SOLID_COLOR mode.
  {
    fuchsia_ui_composition::LayerProperties props;
    props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    props.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 1.f});
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    EXPECT_TRUE(std::holds_alternative<UberStructLayer::SolidColorModeProperties>(
        uber_struct->layers.at(layer_handle).content));
  }

  // Call carrying only properties (no composition_mode) leaves mode unchanged.
  {
    fuchsia_ui_composition::LayerProperties props;
    props.opacity(0.5f);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.opacity, 0.5f);
    EXPECT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
  }

  // Call carrying only composition_mode switches mode leaving stored properties intact.
  {
    fuchsia_ui_composition::LayerProperties props;
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.opacity, 0.5f);
    EXPECT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
  }

  // Call carrying both properties and composition_mode applies both together.
  {
    fuchsia_ui_composition::LayerProperties props;
    props.color(fuchsia_ui_composition::ColorRgba{0.f, 1.f, 0.f, 1.f});
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    EXPECT_EQ(std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content).color,
              (std::array<float, 4>{0.f, 1.f, 0.f, 1.f}));
  }
}

TEST_F(Flatland2Test, ResetLayerForgetsEverything) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // Fully populate layer.
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{10, 20, 30, 40});
  props.opacity(0.3f);
  props.blend_mode(fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha);
  props.color(fuchsia_ui_composition::ColorRgba{0.1f, 0.2f, 0.3f, 0.4f});
  props.sample_rect(fuchsia_math::RectF{1.f, 2.f, 3.f, 4.f});
  props.transform(fuchsia_ui_composition::FlipThenRotate::kFlipH);
  props.hint_damage_rects({{fuchsia_math::RectU{0, 0, 10, 10}}});
  props.hint_visible_rects({{fuchsia_math::RectU{0, 0, 20, 20}}});
  props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));

  Present(flatland, true);

  // ResetLayer.
  flatland->ResetLayer(kLayer);
  EXPECT_FALSE(error_log.has_value());

  auto* obj = flatland->GetLayerObjectForTest(layer_handle);
  ASSERT_NE(obj, nullptr);
  EXPECT_TRUE(obj->hint_damage_rects.empty());
  EXPECT_TRUE(obj->hint_visible_rects.empty());
  EXPECT_EQ(obj->mode, LayerObject::Mode::kInvisible);

  Present(flatland, true);

  // In snapshot, mode is monostate (invisible), common properties are reset to defaults.
  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.display_rect, types::Rectangle{});
    EXPECT_EQ(us_layer.common.opacity, 1.0f);
    EXPECT_EQ(us_layer.common.blend_mode, types::BlendMode::kReplace());
    EXPECT_TRUE(std::holds_alternative<std::monostate>(us_layer.content));
  }

  // Entering solid-color mode now sees defaults, not stale values.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    EXPECT_EQ(std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content).color,
              (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
  }

  // Entering image mode now sees defaults, not stale values.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
    const auto& img = std::get<UberStructLayer::ImageModeProperties>(us_layer.content);
    EXPECT_EQ(img.sample_rect, types::RectangleF{});
    EXPECT_EQ(img.transform, types::RotateFlip::kIdentity());
  }
}

TEST_F(Flatland2Test, InvisibleRoundTripLosesNothing) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // 1. Fully configure layer in SOLID_COLOR mode.
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{5, 15, 25, 35});
  props.opacity(0.85f);
  props.color(fuchsia_ui_composition::ColorRgba{0.3f, 0.6f, 0.9f, 1.f});
  props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));

  Present(flatland, true);

  // 2. Switch to INVISIBLE mode.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kInvisible);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    EXPECT_TRUE(
        std::holds_alternative<std::monostate>(uber_struct->layers.at(layer_handle).content));
    EXPECT_TRUE(ComputeResolvedLayers(flatland.get()).empty());
  }

  // 3. Switch back to SOLID_COLOR mode without re-sending any properties.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    EXPECT_EQ(us_layer.common.display_rect,
              (types::Rectangle({.x = 5, .y = 15, .width = 25, .height = 35})));
    EXPECT_EQ(us_layer.common.opacity, 0.85f);
    ASSERT_TRUE(
        std::holds_alternative<UberStructLayer::SolidColorModeProperties>(us_layer.content));
    EXPECT_EQ(std::get<UberStructLayer::SolidColorModeProperties>(us_layer.content).color,
              (std::array<float, 4>{0.3f, 0.6f, 0.9f, 1.f}));
  }
}

TEST_F(Flatland2Test, InvisibleLayerInStack) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer1(2);
  const LayerId kLayer2(3);
  const LayerId kLayer3(4);
  const LayerStackId kStack(5);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer1);
  flatland->CreateLayer(kLayer2);
  flatland->CreateLayer(kLayer3);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer1, kLayer2, kLayer3});
  flatland->SetTransformContent(kRoot, kStack);

  // Layer 1: Solid Red
  fuchsia_ui_composition::LayerProperties props1;
  props1.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props1.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 1.f});
  props1.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer1, fidl::ToWire(arena, std::move(props1)));

  // Layer 2: Invisible
  fuchsia_ui_composition::LayerProperties props2;
  props2.display_rect(fuchsia_math::RectU{0, 0, 50, 50});
  props2.composition_mode(fuchsia_ui_composition::CompositionMode::kInvisible);
  flatland->SetLayerProperties(kLayer2, fidl::ToWire(arena, std::move(props2)));

  // Layer 3: Solid Green
  fuchsia_ui_composition::LayerProperties props3;
  props3.display_rect(fuchsia_math::RectU{10, 10, 80, 80});
  props3.color(fuchsia_ui_composition::ColorRgba{0.f, 1.f, 0.f, 1.f});
  props3.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer3, fidl::ToWire(arena, std::move(props3)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);
  auto handle2 = flatland->GetLayerHandleForTest(kLayer2);
  ASSERT_TRUE(uber_struct->layers.contains(handle2));
  EXPECT_TRUE(std::holds_alternative<std::monostate>(uber_struct->layers.at(handle2).content));

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 2u);

  // Scene dump prints without crashing.
  std::ostringstream str;
  auto snapshot = uber_struct_system_->Snapshot();
  auto links = link_system_->GetResolvedTopologyLinks();
  auto root_transform = flatland->GetRoot();
  auto topology_data = GlobalTopologyData::ComputeGlobalTopologyData(
      snapshot.map, links, link_system_->GetInstanceId(), root_transform);
  DumpScene(snapshot.map, topology_data, resolved_layers, str);
  EXPECT_FALSE(str.str().empty());

  // Layer 1 (Red)
  EXPECT_EQ(resolved_layers[0].geometry, (SrcToDest(types::RectangleF({0, 0, 100, 100}))));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[0].content).color,
            (std::array<float, 4>{1.f, 0.f, 0.f, 1.f}));
  // Layer 3 (Green)
  EXPECT_EQ(resolved_layers[1].geometry, (SrcToDest(types::RectangleF({10, 10, 80, 80}))));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[1].content).color,
            (std::array<float, 4>{0.f, 1.f, 0.f, 1.f}));
}

TEST_F(Flatland2Test, StraightAlphaSolidNormalizedAtResolve) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerId kLayerB(3);
  const LayerStackId kStack(4);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});
  flatland->SetTransformContent(kRoot, kStack);

  // Layer A: set STRAIGHT_ALPHA directly in solid mode.
  fuchsia_ui_composition::LayerProperties props_a;
  props_a.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props_a.blend_mode(fuchsia_ui_composition::BlendMode2::kStraightAlpha);
  props_a.color(fuchsia_ui_composition::ColorRgba{1.f, 0.5f, 0.25f, 0.5f});
  props_a.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayerA, fidl::ToWire(arena, std::move(props_a)));

  // Layer B: set STRAIGHT_ALPHA while in IMAGE mode, then switch to SOLID_COLOR mode.
  fuchsia_ui_composition::LayerProperties props_b_image;
  props_b_image.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props_b_image.blend_mode(fuchsia_ui_composition::BlendMode2::kStraightAlpha);
  props_b_image.color(fuchsia_ui_composition::ColorRgba{1.f, 0.5f, 0.25f, 0.5f});
  props_b_image.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayerB, fidl::ToWire(arena, std::move(props_b_image)));

  fuchsia_ui_composition::LayerProperties props_b_solid;
  props_b_solid.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayerB, fidl::ToWire(arena, std::move(props_b_solid)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);
  auto handle_a = flatland->GetLayerHandleForTest(kLayerA);
  auto handle_b = flatland->GetLayerHandleForTest(kLayerB);

  // Snapshot holds kStraightAlpha verbatim for both layers.
  EXPECT_EQ(uber_struct->layers.at(handle_a).common.blend_mode, types::BlendMode::kStraightAlpha());
  EXPECT_EQ(uber_struct->layers.at(handle_b).common.blend_mode, types::BlendMode::kStraightAlpha());

  // Resolved layers emit kPremultipliedAlpha and premultiplied content color for both layers.
  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 2u);
  const std::array<float, 4> expected_color = {0.5f, 0.25f, 0.125f, 0.5f};

  EXPECT_EQ(resolved_layers[0].blend_mode, types::BlendMode::kPremultipliedAlpha());
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[0].content).color,
            expected_color);

  EXPECT_EQ(resolved_layers[1].blend_mode, types::BlendMode::kPremultipliedAlpha());
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[1].content).color,
            expected_color);
}

TEST_F(Flatland2Test, DefaultsOnFirstModeEntry) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  // Fresh layer entering SOLID_COLOR mode with no color authored.
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 1u);
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[0].content).color,
            (std::array<float, 4>{1.f, 1.f, 1.f, 1.f}));
}

TEST_F(Flatland2Test, TransformStoredAsRotateFlip) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  fidl::Arena arena;
  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  using fuchsia_ui_composition::FlipThenRotate;
  const std::vector<std::pair<FlipThenRotate, types::RotateFlip>> cases = {
      {FlipThenRotate(0), types::RotateFlip::kIdentity()},
      {FlipThenRotate::kFlipH, types::RotateFlip::kReflectY()},
      {FlipThenRotate::kFlipV, types::RotateFlip::kReflectX()},
      {FlipThenRotate::kFlipH | FlipThenRotate::kFlipV, types::RotateFlip::kRotateCcw180()},
      {FlipThenRotate::kRotate90, types::RotateFlip::kRotateCcw270()},
      {FlipThenRotate::kFlipH | FlipThenRotate::kRotate90,
       types::RotateFlip::kRotateCcw90ReflectY()},
      {FlipThenRotate::kFlipV | FlipThenRotate::kRotate90,
       types::RotateFlip::kRotateCcw90ReflectX()},
      {FlipThenRotate::kFlipH | FlipThenRotate::kFlipV | FlipThenRotate::kRotate90,
       types::RotateFlip::kRotateCcw90()},
  };

  for (const auto& [flip_then_rotate, expected_rf] : cases) {
    fuchsia_ui_composition::LayerProperties props;
    props.transform(flip_then_rotate);
    props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
    Present(flatland, true);

    auto uber_struct = GetUberStruct(flatland.get());
    ASSERT_NE(uber_struct, nullptr);
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const auto& us_layer = uber_struct->layers.at(layer_handle);
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
    EXPECT_EQ(std::get<UberStructLayer::ImageModeProperties>(us_layer.content).transform,
              expected_rf);
  }
}

TEST_F(Flatland2Test, SetLayerPropertiesStoresHintRects) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  const fuchsia_math::RectU kDamageRect{0, 0, 10, 10};
  const fuchsia_math::RectU kVisibleRect{0, 0, 20, 20};

  fuchsia_ui_composition::LayerProperties props;
  props.hint_damage_rects({{kDamageRect}});
  props.hint_visible_rects({{kVisibleRect}});
  fidl::Arena arena;
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  EXPECT_FALSE(error_log.has_value());

  auto* obj = flatland->GetLayerObjectForTest(flatland->GetLayerHandleForTest(kLayer));
  ASSERT_NE(obj, nullptr);
  EXPECT_EQ(obj->hint_damage_rects,
            std::vector<types::Rectangle>{types::Rectangle::From(kDamageRect)});
  EXPECT_EQ(obj->hint_visible_rects,
            std::vector<types::Rectangle>{types::Rectangle::From(kVisibleRect)});
}

TEST_F(Flatland2Test, SetLayerPropertiesUnknownLayerFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  fidl::Arena arena;
  flatland->SetLayerProperties(LayerId(1), fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetLayerPropertiesFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  fidl::Arena arena;
  flatland->SetLayerProperties(LayerId(1), fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidDisplayRectFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{std::numeric_limits<uint32_t>::max(), 0, 100, 100});
  fidl::Arena arena;
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidOpacityFails) {
  for (float opacity : {-0.1f, 1.1f, std::numeric_limits<float>::quiet_NaN(),
                        std::numeric_limits<float>::infinity()}) {
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    const LayerId kLayer(1);
    flatland->CreateLayer(kLayer);

    fuchsia_ui_composition::LayerProperties props;
    props.opacity(opacity);
    fidl::Arena arena;
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
    ASSERT_TRUE(error_log.has_value());
    Present(flatland, false);
  }
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidColorFails) {
  const std::vector<fuchsia_ui_composition::ColorRgba> bad_colors = {
      fuchsia_ui_composition::ColorRgba{-0.1f, 0.f, 0.f, 1.f},
      fuchsia_ui_composition::ColorRgba{1.1f, 0.f, 0.f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, -0.1f, 0.f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, 1.1f, 0.f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, 0.f, -0.1f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, 0.f, 1.1f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, 0.f, 0.f, -0.1f},
      fuchsia_ui_composition::ColorRgba{0.f, 0.f, 0.f, 1.1f},
      fuchsia_ui_composition::ColorRgba{std::numeric_limits<float>::quiet_NaN(), 0.f, 0.f, 1.f},
      fuchsia_ui_composition::ColorRgba{0.f, std::numeric_limits<float>::infinity(), 0.f, 1.f},
  };

  for (const auto& color : bad_colors) {
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    const LayerId kLayer(1);
    flatland->CreateLayer(kLayer);

    fuchsia_ui_composition::LayerProperties props;
    props.color(color);
    fidl::Arena arena;
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
    ASSERT_TRUE(error_log.has_value());
    Present(flatland, false);
  }
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidSampleRectFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  fuchsia_ui_composition::LayerProperties props;
  props.sample_rect(fuchsia_math::RectF{0.f, 0.f, -10.f, 10.f});
  fidl::Arena arena;
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidDamageHintsFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  fuchsia_ui_composition::LayerProperties props;
  props.hint_damage_rects(
      {{fuchsia_math::RectU{std::numeric_limits<uint32_t>::max(), 0, 100, 100}}});
  fidl::Arena arena;
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, SetLayerPropertiesInvalidVisibleHintsFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  fuchsia_ui_composition::LayerProperties props;
  props.hint_visible_rects(
      {{fuchsia_math::RectU{std::numeric_limits<uint32_t>::max(), 0, 100, 100}}});
  fidl::Arena arena;
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ResetLayerUnknownLayerFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  flatland->ResetLayer(LayerId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, ResetLayerFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));
  flatland->ResetLayer(LayerId(1));
  ASSERT_TRUE(error_log.has_value());
  Present(flatland, false);
}

TEST_F(Flatland2Test, TranslucentReplaceSolidCulls) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayerA(2);
  const LayerId kLayerB(3);
  const LayerStackId kStack(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);

  flatland->CreateLayer(kLayerA);
  flatland->CreateLayer(kLayerB);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayerA, kLayerB});
  flatland->SetTransformContent(kRoot, kStack);

  // Layer A (bottom): full-screen 100x100, opaque blue, REPLACE, opacity 1.0.
  fidl::Arena arena;
  fuchsia_ui_composition::LayerProperties props_a;
  props_a.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props_a.color(fuchsia_ui_composition::ColorRgba{0.f, 0.f, 1.f, 1.f});
  props_a.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
  props_a.opacity(1.0f);
  props_a.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayerA, fidl::ToWire(arena, std::move(props_a)));

  // Layer B (top): full-screen 100x100, translucent red (alpha 0.5), REPLACE, opacity 1.0.
  fuchsia_ui_composition::LayerProperties props_b;
  props_b.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props_b.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 0.5f});
  props_b.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
  props_b.opacity(1.0f);
  props_b.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayerB, fidl::ToWire(arena, std::move(props_b)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  // A REPLACE solid with color.a < 1.0 and opacity == 1.0 retains REPLACE blend mode
  // (this is the hole-punch path) and therefore occludes/culls the layer underneath.
  auto renderables = GetRenderables(flatland.get(), 100, 100);
  ASSERT_EQ(renderables.size(), 1u);
  EXPECT_EQ(renderables[0].blend_mode, types::BlendMode::kReplace());
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(renderables[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(renderables[0].content).color,
            (std::array<float, 4>{0.5f, 0.f, 0.f, 0.5f}));
}

TEST_F(Flatland2Test, OpacityDemotesReplace) {
  {
    // Sub-case 1: Layer-level opacity < 1.0 demotes REPLACE to PREMULTIPLIED_ALPHA -> lower layer
    // survives.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    const TransformId kRoot(1);
    const LayerId kLayerA(2);
    const LayerId kLayerB(3);
    const LayerStackId kStack(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayerA);
    flatland->CreateLayer(kLayerB);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayerA, kLayerB});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    fuchsia_ui_composition::LayerProperties props_a;
    props_a.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    props_a.color(fuchsia_ui_composition::ColorRgba{0.f, 0.f, 1.f, 1.f});
    props_a.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
    props_a.opacity(1.0f);
    props_a.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayerA, fidl::ToWire(arena, std::move(props_a)));

    fuchsia_ui_composition::LayerProperties props_b;
    props_b.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    props_b.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 0.5f});
    props_b.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
    props_b.opacity(0.5f);  // Layer opacity < 1.0!
    props_b.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayerB, fidl::ToWire(arena, std::move(props_b)));

    Present(flatland, true);

    auto renderables = GetRenderables(flatland.get(), 100, 100);
    ASSERT_EQ(renderables.size(), 2u);
    EXPECT_EQ(renderables[1].blend_mode, types::BlendMode::kPremultipliedAlpha());
  }

  {
    // Sub-case 2: Ancestor transform opacity < 1.0 demotes REPLACE to PREMULTIPLIED_ALPHA -> lower
    // layer survives.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    const TransformId kRoot(1);
    const TransformId kChild(2);
    const LayerId kLayerA(3);
    const LayerId kLayerB(4);
    const LayerStackId kStackA(5);
    const LayerStackId kStackB(6);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateTransform(kChild);
    flatland->AddChild(kRoot, kChild);
    flatland->SetOpacity(kChild, 0.5f);  // Ancestor transform opacity < 1.0!

    flatland->CreateLayer(kLayerA);
    flatland->CreateLayer(kLayerB);
    flatland->CreateLayerStack(kStackA);
    flatland->CreateLayerStack(kStackB);
    flatland->SetStackLayers(kStackA, {kLayerA});
    flatland->SetStackLayers(kStackB, {kLayerB});
    flatland->SetTransformContent(kRoot, kStackA);
    flatland->SetTransformContent(kChild, kStackB);

    fidl::Arena arena;
    fuchsia_ui_composition::LayerProperties props_a;
    props_a.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    props_a.color(fuchsia_ui_composition::ColorRgba{0.f, 0.f, 1.f, 1.f});
    props_a.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
    props_a.opacity(1.0f);
    props_a.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayerA, fidl::ToWire(arena, std::move(props_a)));

    fuchsia_ui_composition::LayerProperties props_b;
    props_b.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    props_b.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 0.5f});
    props_b.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
    props_b.opacity(1.0f);  // Layer opacity is 1.0, but effective opacity is 0.5.
    props_b.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayerB, fidl::ToWire(arena, std::move(props_b)));

    Present(flatland, true);

    auto renderables = GetRenderables(flatland.get(), 100, 100);
    ASSERT_EQ(renderables.size(), 2u);
    EXPECT_EQ(renderables[1].blend_mode, types::BlendMode::kPremultipliedAlpha());
  }
}

TEST_F(Flatland2Test, SolidColorLayerRenders) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  fuchsia_ui_composition::LayerProperties props;
  props.display_rect(fuchsia_math::RectU{10, 20, 50, 60});
  props.color(fuchsia_ui_composition::ColorRgba{0.2f, 0.4f, 0.6f, 1.0f});
  props.blend_mode(fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha);
  props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(props)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 1u);
  EXPECT_EQ(resolved_layers[0].geometry, (SrcToDest(types::RectangleF({10, 20, 50, 60}))));
  EXPECT_EQ(resolved_layers[0].blend_mode, types::BlendMode::kPremultipliedAlpha());
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(resolved_layers[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[0].content).color,
            (std::array<float, 4>{0.2f, 0.4f, 0.6f, 1.0f}));
}

TEST_F(Flatland2Test, TwoSolidLayersZOrder) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const TransformId kRoot(1);
  const LayerId kLayer1(2);
  const LayerId kLayer2(3);
  const LayerStackId kStack(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer1);
  flatland->CreateLayer(kLayer2);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer1, kLayer2});
  flatland->SetTransformContent(kRoot, kStack);

  // Layer 1: Red
  fidl::Arena arena;
  fuchsia_ui_composition::LayerProperties props1;
  props1.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props1.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 1.f});
  props1.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer1, fidl::ToWire(arena, std::move(props1)));

  // Layer 2: Blue
  fuchsia_ui_composition::LayerProperties props2;
  props2.display_rect(fuchsia_math::RectU{10, 10, 50, 50});
  props2.color(fuchsia_ui_composition::ColorRgba{0.f, 0.f, 1.f, 1.f});
  props2.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
  flatland->SetLayerProperties(kLayer2, fidl::ToWire(arena, std::move(props2)));

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 2u);
  // Layer 1 is back-most (index 0)
  EXPECT_EQ(resolved_layers[0].geometry, (SrcToDest(types::RectangleF({0, 0, 100, 100}))));
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(resolved_layers[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[0].content).color,
            (std::array<float, 4>{1.f, 0.f, 0.f, 1.f}));

  // Layer 2 is front-most (index 1)
  EXPECT_EQ(resolved_layers[1].geometry, (SrcToDest(types::RectangleF({10, 10, 50, 50}))));
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::SolidColorContent>(resolved_layers[1].content));
  EXPECT_EQ(std::get<ResolvedLayer::SolidColorContent>(resolved_layers[1].content).color,
            (std::array<float, 4>{0.f, 0.f, 1.f, 1.f}));
}

TEST_F(Flatland2Test, CreateImage2ImportsAndRegisters) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);
  const LayerId kLayerId(1);  // Coincides with image id to test per-type id spaces.

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  flatland->CreateLayer(kLayerId);

  EXPECT_FALSE(error_log.has_value());

  auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id, allocation::kInvalidImageId);

  const auto* img_obj = flatland->GetImageObjectForTest(global_id);
  ASSERT_NE(img_obj, nullptr);
  EXPECT_EQ(img_obj->ref_count, 1u);
  EXPECT_EQ(img_obj->metadata.identifier, global_id);
  EXPECT_EQ(img_obj->metadata.width, 100u);
  EXPECT_EQ(img_obj->metadata.height, 200u);
  EXPECT_EQ(img_obj->metadata.vmo_index, 0u);
}

TEST_F(Flatland2Test, CreateImage2ImportFailureCleansUp) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);

  // Return failure on import.
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::error()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);

  const auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id, allocation::kInvalidImageId);
  EXPECT_NE(flatland->GetImageObjectForTest(global_id), nullptr);

  // Run the loop so the promise resolves and fails.
  RunLoopUntilIdle();

  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("Importer could not import image"), std::string::npos);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, CreateImage2ImportFailureUnbindsLayersAndCleansUp) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);
  const LayerId kLayer1(2);
  const LayerId kLayer2(3);

  // Return failure on import.
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::error()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  flatland->CreateLayer(kLayer1);
  flatland->CreateLayer(kLayer2);

  const auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id, allocation::kInvalidImageId);

  LayerHandle handle1 = flatland->GetLayerHandleForTest(kLayer1);
  LayerHandle handle2 = flatland->GetLayerHandleForTest(kLayer2);

  // Before the async import promise resolves, bind the image to two layers AND release the client
  // ImageId. The image object is now kept alive solely by the two layer bindings (ref_count == 2).
  flatland->SetLayerImageForTest(handle1, global_id);
  flatland->SetLayerImageForTest(handle2, global_id);
  flatland->ReleaseImage2(kImageId);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Run the loop so the promise resolves and fails. Both layers must be unbound and the image
  // object destroyed.
  RunLoopUntilIdle();

  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("Importer could not import image"), std::string::npos);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle1)->image_mode.image_id,
            allocation::kInvalidImageId);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle2)->image_mode.image_id,
            allocation::kInvalidImageId);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, ReleaseImage2FreesUnboundImage) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  RunLoopUntilIdle();

  EXPECT_FALSE(error_log.has_value());
  auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id, allocation::kInvalidImageId);

  // Releasing image decrements ref_count to 0 and immediately frees the client id.
  flatland->ReleaseImage2(kImageId);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);

  // Client ID kImageId can be reused immediately.
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));
  auto properties2 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                         .size(fuchsia_math::wire::SizeU{50, 60})
                         .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties2);
  RunLoopUntilIdle();
  EXPECT_FALSE(error_log.has_value());
  auto global_id2 = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id2, allocation::kInvalidImageId);
  EXPECT_NE(global_id2, global_id);

  // The first image is queued for release behind the next Present's release fence.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(0);
  Present(flatland, true);

  // Signal the release fence: ReleaseBufferImage(global_id) is called once.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  // The Flatland destructor will release the second image.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id2)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, ImageObjectRefCountLifecycle) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);
  const LayerId kLayer1(2);
  const LayerId kLayer2(3);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  RunLoopUntilIdle();
  flatland->CreateLayer(kLayer1);
  flatland->CreateLayer(kLayer2);

  auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_id, allocation::kInvalidImageId);
  auto* img_obj = flatland->GetImageObjectForTest(global_id);
  ASSERT_NE(img_obj, nullptr);
  EXPECT_EQ(img_obj->ref_count, 1u);

  LayerHandle handle1 = flatland->GetLayerHandleForTest(kLayer1);
  LayerHandle handle2 = flatland->GetLayerHandleForTest(kLayer2);

  // Bind to Layer 1 via test seam.
  flatland->SetLayerImageForTest(handle1, global_id);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Bind to Layer 2 via test seam.
  flatland->SetLayerImageForTest(handle2, global_id);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 3u);

  // Releasing the client ImageId decrements ref_count to 2; image object survives!
  flatland->ReleaseImage2(kImageId);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Destroy Layer 1: decrements ref_count to 1; image object survives!
  flatland->ReleaseLayer(kLayer1);
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 1u);

  // Destroy Layer 2: decrements ref_count to 0; image object erased!
  flatland->ReleaseLayer(kLayer2);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);

  // Deferred release observed after Present + release fence signal!
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(0);
  Present(flatland, true);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, ResetLayerUnbindsAndReleasesImage) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);
  const LayerId kLayerId(2);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  RunLoopUntilIdle();
  flatland->CreateLayer(kLayerId);

  auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  LayerHandle handle = flatland->GetLayerHandleForTest(kLayerId);

  flatland->SetLayerImageForTest(handle, global_id);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Release client ImageId; image survives because it is bound to kLayerId.
  flatland->ReleaseImage2(kImageId);
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 1u);

  // ResetLayer must unbind the image and drop its ref count to 0.
  flatland->ResetLayer(kLayerId);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetLayerObjectForTest(handle)->image_mode.image_id,
            allocation::kInvalidImageId);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(0);
  Present(flatland, true);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

// Verifies the transitive lifetime chain: LayerStack -> LayerObject -> ImageObject.
// Even after the client releases both the ImageId (via ReleaseImage2) and LayerId
// (via ReleaseLayer), the stack continues to hold a reference to the LayerObject, which
// in turn transitively keeps the bound ImageObject alive across presents.
// Once the layer is removed from the stack, the LayerObject is destroyed, which unbinds
// the image, drops the ImageObject's ref count to 0, and queues it for deferred release
// upon the next Present.
TEST_F(Flatland2Test, StackKeepsLayerKeepsImageAlive) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const ImageId kImageId(1);
  const LayerId kLayerId(2);
  const LayerStackId kStackId(3);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  RunLoopUntilIdle();
  flatland->CreateLayer(kLayerId);
  flatland->CreateLayerStack(kStackId);

  auto global_id = flatland->GetGlobalImageIdForTest(kImageId);
  LayerHandle layer_handle = flatland->GetLayerHandleForTest(kLayerId);

  // Bind image to layer, and attach layer to stack.
  flatland->SetLayerImageForTest(layer_handle, global_id);
  flatland->SetStackLayers(kStackId, {kLayerId});

  // Release BOTH the client ImageId and client LayerId.
  // The stack keeps the LayerObject alive (layer ref_count == 1), which in turn keeps the
  // ImageObject alive (image ref_count == 1).
  flatland->ReleaseImage2(kImageId);
  flatland->ReleaseLayer(kLayerId);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  EXPECT_EQ(flatland->GetLayerHandleForTest(kLayerId), LayerHandle());
  ASSERT_NE(flatland->GetLayerObjectForTest(layer_handle), nullptr);
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 1u);

  // Present while attached to stack: image is NOT released.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(0);
  Present(flatland, true);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  // Removing the layer from the stack destroys the LayerObject, which unbinds and destroys the
  // ImageObject.
  flatland->SetStackLayers(kStackId, {});
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_handle), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);

  // Image release is now observed after the next Present retires.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(0);
  Present(flatland, true);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, TeardownReleasesLiveImageObjects) {
  std::shared_ptr<Allocator> allocator = CreateAllocator();
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  allocation::GlobalImageId id1 = allocation::kInvalidImageId;
  allocation::GlobalImageId id2 = allocation::kInvalidImageId;

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(testing::Invoke(
          [&id1](const ImageMetadata& metadata, allocation::BufferCollectionUsage usage_type) {
            id1 = metadata.identifier;
            return fpromise::make_ok_promise();
          }))
      .WillOnce(testing::Invoke(
          [&id2](const ImageMetadata& metadata, allocation::BufferCollectionUsage usage_type) {
            id2 = metadata.identifier;
            return fpromise::make_ok_promise();
          }));

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 200})
                        .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  flatland->CreateImage2(ImageId(2), ToWire(ref_pair.DuplicateImportToken()), 0, properties);

  RunLoopUntilIdle();
  EXPECT_NE(id1, allocation::kInvalidImageId);
  EXPECT_NE(id2, allocation::kInvalidImageId);

  // Destroy the flatland session with live image objects. Both should be released by importers.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(id1)).Times(1);
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(id2)).Times(1);

  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, CreateImage2FailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));

  auto ref_pair = BufferCollectionImportExportTokens::New();
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("Flatland2 not enabled"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2ZeroIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();
  flatland->CreateImage2(kInvalidImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("image_id 0"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2DuplicateIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  fidl::Arena arena;
  auto properties1 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                         .size(fuchsia_math::wire::SizeU{100, 100})
                         .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties1);
  EXPECT_FALSE(error_log.has_value());

  auto properties2 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                         .size(fuchsia_math::wire::SizeU{100, 100})
                         .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties2);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("pre-existing image_id"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2InvalidExportTokenFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);

  // Unregistered / invalid export token.
  fuchsia_ui_composition::BufferCollectionImportToken import_token;
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();
  flatland->CreateImage2(ImageId(1), ToWire(std::move(import_token)), 0, properties);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("no valid export token"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2NoSizeFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, {});
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("did not specify size"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2ZeroWidthFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{0, 100})
                        .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("did not specify a width"), std::string::npos);
}

TEST_F(Flatland2Test, CreateImage2ZeroHeightFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 0})
                        .Build();
  flatland->CreateImage2(ImageId(1), ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("did not specify a height"), std::string::npos);
}

TEST_F(Flatland2Test, ReleaseImage2FailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));

  flatland->ReleaseImage2(ImageId(1));
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("Flatland2 not enabled"), std::string::npos);
}

TEST_F(Flatland2Test, ReleaseImage2ZeroIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);

  flatland->ReleaseImage2(kInvalidImageId);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("image_id 0"), std::string::npos);
}

TEST_F(Flatland2Test, ReleaseImage2UnknownIdFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);

  flatland->ReleaseImage2(ImageId(999));
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("not found"), std::string::npos);
}

TEST_F(Flatland2Test, ClearResetsFlatland2Ids) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  const LayerId kLayerId(1);
  const LayerStackId kStackId(2);
  const ImageId kImageId(3);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  flatland->CreateLayer(kLayerId);
  flatland->CreateLayerStack(kStackId);
  fidl::Arena arena;
  auto properties = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                        .size(fuchsia_math::wire::SizeU{100, 100})
                        .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties);
  EXPECT_FALSE(error_log.has_value());

  LayerHandle layer_handle = flatland->GetLayerHandleForTest(kLayerId);
  ASSERT_NE(layer_handle, LayerHandle());
  EXPECT_NE(flatland->GetLayerObjectForTest(layer_handle), nullptr);

  auto stack_handle = flatland->GetLayerStackHandleForTest(kStackId);
  ASSERT_TRUE(stack_handle.has_value());

  allocation::GlobalImageId global_image_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(global_image_id, allocation::kInvalidImageId);
  EXPECT_NE(flatland->GetImageObjectForTest(global_image_id), nullptr);

  flatland->Clear();

  // The client-only layer is destroyed by the Clear().
  EXPECT_EQ(flatland->GetLayerObjectForTest(layer_handle), nullptr);
  EXPECT_EQ(flatland->GetLayerHandleForTest(kLayerId), LayerHandle());
  EXPECT_FALSE(flatland->GetLayerStackHandleForTest(kStackId).has_value());
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageId), allocation::kInvalidImageId);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_image_id), nullptr);

  // Recreate all three IDs: succeeds.
  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  flatland->CreateLayer(kLayerId);
  flatland->CreateLayerStack(kStackId);
  auto properties2 = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                         .size(fuchsia_math::wire::SizeU{100, 100})
                         .Build();
  flatland->CreateImage2(kImageId, ToWire(ref_pair.DuplicateImportToken()), 0, properties2);
  EXPECT_FALSE(error_log.has_value());

  LayerHandle new_layer_handle = flatland->GetLayerHandleForTest(kLayerId);
  EXPECT_NE(new_layer_handle, LayerHandle());
  EXPECT_NE(new_layer_handle, layer_handle);
  EXPECT_NE(flatland->GetLayerObjectForTest(new_layer_handle), nullptr);

  auto new_stack_handle = flatland->GetLayerStackHandleForTest(kStackId);
  ASSERT_TRUE(new_stack_handle.has_value());
  EXPECT_NE(*new_stack_handle, *stack_handle);

  allocation::GlobalImageId new_global_image_id = flatland->GetGlobalImageIdForTest(kImageId);
  EXPECT_NE(new_global_image_id, allocation::kInvalidImageId);
  EXPECT_NE(new_global_image_id, global_image_id);
  EXPECT_NE(flatland->GetImageObjectForTest(new_global_image_id), nullptr);

  // The unbound image's release is observed at the next Present()'s fence.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(_)).Times(0);
  Present(flatland, true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(global_image_id)).Times(1);
  ApplySessionUpdatesAndSignalFences();
  RunLoopUntilIdle();

  // The Flatland destructor will release the recreated image.
  EXPECT_CALL(*mock_buffer_collection_importer_, ReleaseBufferImage(new_global_image_id)).Times(1);
  flatland.reset();
  RunLoopUntilIdle();
}

TEST_F(Flatland2Test, BindResolvesIntoSnapshot) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImage(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto img_props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                       .size(fuchsia_math::wire::SizeU{100, 200})
                       .Build();
  flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, img_props);

  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 200});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto global_id = flatland->GetGlobalImageIdForTest(kImage);
  EXPECT_NE(global_id, allocation::kInvalidImageId);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);
  auto uber_struct = GetUberStruct(flatland.get());
  ASSERT_NE(uber_struct, nullptr);
  ASSERT_TRUE(uber_struct->layers.contains(layer_handle));

  const auto& us_layer = uber_struct->layers.at(layer_handle);
  ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(us_layer.content));
  const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(us_layer.content);
  EXPECT_EQ(img_content.image_id, global_id);
  EXPECT_EQ(img_content.image_width, 100u);
  EXPECT_EQ(img_content.image_height, 200u);
  EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 100.f, 200.f})));

  auto renderables = GetRenderables(flatland.get(), 100, 200);
  ASSERT_EQ(renderables.size(), 1u);
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(renderables[0].content));
  const auto& resolved_img = std::get<ResolvedLayer::ImageContent>(renderables[0].content);
  EXPECT_EQ(resolved_img.image_id, global_id);
  EXPECT_EQ(resolved_img.width, 100u);
  EXPECT_EQ(resolved_img.height, 200u);
}

TEST_F(Flatland2Test, ImageCyclingStableLayer) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_a.export_token), CreateToken(), true);
  }
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_b.export_token), CreateToken(), true);
  }

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillRepeatedly(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImageA(4);
  const ImageId kImageB(5);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props_a = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
  flatland->CreateImage2(kImageA, ToWire(ref_pair_a.DuplicateImportToken()), 0, props_a);

  auto props_b = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{200, 200})
                     .Build();
  flatland->CreateImage2(kImageB, ToWire(ref_pair_b.DuplicateImportToken()), 0, props_b);

  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  auto global_a = flatland->GetGlobalImageIdForTest(kImageA);
  auto global_b = flatland->GetGlobalImageIdForTest(kImageB);
  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // 1. Bind A -> Present
  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 2u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 1u);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.image_id, global_a);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 100.f, 100.f})));
  }

  // 2. Bind B -> Present
  flatland->SetLayerImage(kLayer, kImageB, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 1u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 2u);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.image_id, global_b);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 200.f, 200.f})));
  }

  // 3. Bind A again -> Present
  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 2u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 1u);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.image_id, global_a);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 100.f, 100.f})));
  }
}

TEST_F(Flatland2Test, SharedImageTwoLayers) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer1(2);
  const LayerId kLayer2(3);
  const LayerStackId kStack(4);
  const ImageId kImage(5);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer1);
  flatland->CreateLayer(kLayer2);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer1, kLayer2});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto img_props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                       .size(fuchsia_math::wire::SizeU{100, 100})
                       .Build();
  flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, img_props);

  fuchsia_ui_composition::LayerProperties props1;
  props1.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  props1.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer1, fidl::ToWire(arena, std::move(props1)));

  fuchsia_ui_composition::LayerProperties props2;
  props2.display_rect(fuchsia_math::RectU{100, 0, 100, 100});
  props2.transform(fuchsia_ui_composition::FlipThenRotate::kFlipH);
  props2.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer2, fidl::ToWire(arena, std::move(props2)));

  flatland->SetLayerImage(kLayer1, kImage, std::nullopt, std::nullopt);
  flatland->SetLayerImage(kLayer2, kImage, std::nullopt, std::nullopt);

  auto global_id = flatland->GetGlobalImageIdForTest(kImage);
  // Ref count: 1 (client) + 2 (layers) = 3
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 3u);

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto resolved_layers = ComputeResolvedLayers(flatland.get());
  ASSERT_EQ(resolved_layers.size(), 2u);

  // Layer 1
  EXPECT_EQ(resolved_layers[0].geometry,
            SrcToDest(types::RectangleF({0, 0, 100, 100}), types::RectangleF({0, 0, 100, 100}),
                      types::RotateFlip::kIdentity()));
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(resolved_layers[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(resolved_layers[0].content).image_id, global_id);

  // Layer 2
  EXPECT_EQ(resolved_layers[1].geometry,
            SrcToDest(types::RectangleF({0, 0, 100, 100}), types::RectangleF({100, 0, 100, 100}),
                      types::RotateFlip::kReflectY()));
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(resolved_layers[1].content));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(resolved_layers[1].content).image_id, global_id);
}

TEST_F(Flatland2Test, RebindReleasesOldImage) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_a.export_token), CreateToken(), true);
  }
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_b.export_token), CreateToken(), true);
  }

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillRepeatedly(ReturnPromise(fpromise::ok()));

  const LayerId kLayer(1);
  const ImageId kImageA(2);
  const ImageId kImageB(3);

  flatland->CreateLayer(kLayer);

  fidl::Arena arena;
  auto props_a = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
  flatland->CreateImage2(kImageA, ToWire(ref_pair_a.DuplicateImportToken()), 0, props_a);

  auto props_b = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{200, 200})
                     .Build();
  flatland->CreateImage2(kImageB, ToWire(ref_pair_b.DuplicateImportToken()), 0, props_b);

  auto global_a = flatland->GetGlobalImageIdForTest(kImageA);
  auto global_b = flatland->GetGlobalImageIdForTest(kImageB);

  // Bind A to layer.
  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 2u);

  // Client releases A. Object survives because layer still holds a reference.
  flatland->ReleaseImage2(kImageA);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImageA), allocation::kInvalidImageId);
  ASSERT_NE(flatland->GetImageObjectForTest(global_a), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 1u);

  // Rebind to B. Old image A has ref_count drop to 0 and is erased.
  flatland->SetLayerImage(kLayer, kImageB, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a), nullptr);
  ASSERT_NE(flatland->GetImageObjectForTest(global_b), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 2u);
}

TEST_F(Flatland2Test, SentinelDefaultAndToggle) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);

  flatland->CreateLayer(kLayer);
  auto handle = flatland->GetLayerHandleForTest(kLayer);
  auto* obj = flatland->GetLayerObjectForTest(handle);
  ASSERT_NE(obj, nullptr);

  // 1. Fresh layer defaults to sample_rect_is_full_image == true.
  EXPECT_TRUE(obj->sample_rect_is_full_image);

  fidl::Arena arena;

  // 2. Concrete sample_rect clears the flag.
  fuchsia_ui_composition::LayerProperties concrete_props;
  concrete_props.sample_rect(fuchsia_math::RectF{10.f, 20.f, 30.f, 40.f});
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(concrete_props)));
  EXPECT_FALSE(obj->sample_rect_is_full_image);
  EXPECT_EQ(obj->image_mode.sample_rect, (types::RectangleF({10.f, 20.f, 30.f, 40.f})));

  // 3. Authoring (0,0,0,0) re-sets the flag.
  fuchsia_ui_composition::LayerProperties sentinel_props;
  sentinel_props.sample_rect(fuchsia_math::RectF{0.f, 0.f, 0.f, 0.f});
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(sentinel_props)));
  EXPECT_TRUE(obj->sample_rect_is_full_image);

  // 4. Concrete rect again -> clear flag -> ResetLayer -> restored to true.
  fuchsia_ui_composition::LayerProperties concrete_props2;
  concrete_props2.sample_rect(fuchsia_math::RectF{5.f, 5.f, 50.f, 50.f});
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(concrete_props2)));
  EXPECT_FALSE(obj->sample_rect_is_full_image);

  flatland->ResetLayer(kLayer);
  EXPECT_TRUE(obj->sample_rect_is_full_image);
}

TEST_F(Flatland2Test, SentinelReexpandsOnRebind) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_a.export_token), CreateToken(), true);
  }
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_b.export_token), CreateToken(), true);
  }

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillRepeatedly(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImageA(4);
  const ImageId kImageB(5);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props_a = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{512, 512})
                     .Build();
  flatland->CreateImage2(kImageA, ToWire(ref_pair_a.DuplicateImportToken()), 0, props_a);

  auto props_b = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{1024, 256})
                     .Build();
  flatland->CreateImage2(kImageB, ToWire(ref_pair_b.DuplicateImportToken()), 0, props_b);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);

  // Set composition_mode to IMAGE without authoring sample_rect (sentinel remains active).
  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 512, 512});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  // 1. Bind A (512x512) -> snapshot shows full extent (0,0,512,512).
  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 512.f, 512.f})));
  }

  // 2. Rebind B (1024x256) -> snapshot tracks to (0,0,1024,256).
  flatland->SetLayerImage(kLayer, kImageB, std::nullopt, std::nullopt);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 1024.f, 256.f})));
  }

  // 3. Author concrete rect -> rebind A does not overwrite it.
  fuchsia_ui_composition::LayerProperties concrete_props;
  concrete_props.sample_rect(fuchsia_math::RectF{10.f, 10.f, 50.f, 50.f});
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(concrete_props)));

  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({10.f, 10.f, 50.f, 50.f})));
  }

  // 4. Author (0,0,0,0) -> back to full extent of image A (0,0,512,512).
  fuchsia_ui_composition::LayerProperties sentinel_props;
  sentinel_props.sample_rect(fuchsia_math::RectF{0.f, 0.f, 0.f, 0.f});
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(sentinel_props)));
  Present(flatland, true);

  {
    auto uber_struct = GetUberStruct(flatland.get());
    const auto& img_content = std::get<UberStructLayer::ImageModeProperties>(
        uber_struct->layers.at(layer_handle).content);
    EXPECT_EQ(img_content.sample_rect, (types::RectangleF({0.f, 0.f, 512.f, 512.f})));
  }
}

TEST_F(Flatland2Test, StompRefCounts) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair_a = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_a.export_token), CreateToken(), true);
  }
  auto ref_pair_b = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_b.export_token), CreateToken(), true);
  }
  auto ref_pair_c = BufferCollectionImportExportTokens::New();
  {
    RegisterBufferCollection(allocator, std::move(ref_pair_c.export_token), CreateToken(), true);
  }

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillRepeatedly(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImageA(4);
  const ImageId kImageB(5);
  const ImageId kImageC(6);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                   .size(fuchsia_math::wire::SizeU{100, 100})
                   .Build();
  flatland->CreateImage2(kImageA, ToWire(ref_pair_a.DuplicateImportToken()), 0, props);
  flatland->CreateImage2(kImageB, ToWire(ref_pair_b.DuplicateImportToken()), 0, props);
  flatland->CreateImage2(kImageC, ToWire(ref_pair_c.DuplicateImportToken()), 0, props);

  auto global_a = flatland->GetGlobalImageIdForTest(kImageA);
  auto global_b = flatland->GetGlobalImageIdForTest(kImageB);
  auto global_c = flatland->GetGlobalImageIdForTest(kImageC);

  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  // Stomp: bind A, then B, then C within the same frame before Present().
  flatland->SetLayerImage(kLayer, kImageA, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 2u);

  flatland->SetLayerImage(kLayer, kImageB, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 1u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 2u);

  flatland->SetLayerImage(kLayer, kImageC, std::nullopt, std::nullopt);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_a)->ref_count, 1u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_b)->ref_count, 1u);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_c)->ref_count, 2u);

  Present(flatland, true);

  auto layer_handle = flatland->GetLayerHandleForTest(kLayer);
  auto uber_struct = GetUberStruct(flatland.get());
  const auto& img_content =
      std::get<UberStructLayer::ImageModeProperties>(uber_struct->layers.at(layer_handle).content);
  EXPECT_EQ(img_content.image_id, global_c);
}

TEST_F(Flatland2Test, BoundImageSurvivesClientReleaseInAnyMode) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImage(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                   .size(fuchsia_math::wire::SizeU{100, 100})
                   .Build();
  flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

  auto global_id = flatland->GetGlobalImageIdForTest(kImage);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Switch to SOLID_COLOR mode.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  // Release client image id while in SOLID_COLOR mode.
  flatland->ReleaseImage2(kImage);
  EXPECT_EQ(flatland->GetGlobalImageIdForTest(kImage), allocation::kInvalidImageId);
  // ImageObject stays alive because the layer's sticky image binding still refs it!
  ASSERT_NE(flatland->GetImageObjectForTest(global_id), nullptr);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 1u);

  // Switch back to IMAGE mode: still displays!
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  auto renderables = GetRenderables(flatland.get(), 100, 100);
  ASSERT_EQ(renderables.size(), 1u);
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(renderables[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(renderables[0].content).image_id, global_id);

  // ResetLayer unbinds and erases the image object.
  flatland->ResetLayer(kLayer);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id), nullptr);
}

TEST_F(Flatland2Test, ReplaceImagePlaneAlpha) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImage(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                   .size(fuchsia_math::wire::SizeU{100, 100})
                   .Build();
  flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  layer_props.blend_mode(fuchsia_ui_composition::BlendMode2::kReplace);
  layer_props.opacity(0.5f);
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto renderables = GetRenderables(flatland.get(), 100, 100);
  ASSERT_EQ(renderables.size(), 1u);
  EXPECT_EQ(renderables[0].blend_mode, types::BlendMode::kPremultipliedAlpha());
  EXPECT_EQ(renderables[0].multiply_color, (std::array<float, 4>{0.5f, 0.5f, 0.5f, 0.5f}));
}

TEST_F(Flatland2Test, SetLayerImageFailsWhenDisabled) {
  std::optional<std::string> error_log;
  auto flatland = FlatlandTest::CreateFlatland();
  flatland->SetErrorReporter(std::make_unique<TestErrorReporter>(error_log));

  flatland->SetLayerImage(LayerId(1), ImageId(1), std::nullopt, std::nullopt);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("Flatland2 not enabled"), std::string::npos);
}

TEST_F(Flatland2Test, SetLayerImageUnknownLayerFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);

  flatland->SetLayerImage(LayerId(999), ImageId(1), std::nullopt, std::nullopt);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("layer 999 not found"), std::string::npos);
}

TEST_F(Flatland2Test, SetLayerImageUnknownImageFails) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  const LayerId kLayer(1);
  flatland->CreateLayer(kLayer);

  flatland->SetLayerImage(kLayer, ImageId(999), std::nullopt, std::nullopt);
  EXPECT_TRUE(error_log.has_value());
  EXPECT_NE(error_log->find("image 999 not found"), std::string::npos);
}

TEST_F(Flatland2Test, SetLayerImageFencesNotYetImplementedFails) {
  {
    // 1. Acquire fence supplied -> rejected with error.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const LayerId kLayer(1);
    const ImageId kImage(2);
    flatland->CreateLayer(kLayer);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    zx::event fence;
    zx_status_t status = zx::event::create(0, &fence);
    ASSERT_EQ(status, ZX_OK);
    fuchsia_ui_composition::wire::WaitFence wait_fence =
        fuchsia_ui_composition::wire::WaitFence::WithBasic(std::move(fence));

    flatland->SetLayerImage(kLayer, kImage, std::move(wait_fence), std::nullopt);

    EXPECT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("fences not yet implemented"), std::string::npos);
  }

  {
    // 2. Release fence supplied -> rejected with error.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const LayerId kLayer(1);
    const ImageId kImage(2);
    flatland->CreateLayer(kLayer);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    zx::event fence;
    zx_status_t status = zx::event::create(0, &fence);
    ASSERT_EQ(status, ZX_OK);
    fuchsia_ui_composition::wire::SignalFence signal_fence =
        fuchsia_ui_composition::wire::SignalFence::WithBasic(std::move(fence));

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::move(signal_fence));

    EXPECT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("fences not yet implemented"), std::string::npos);
  }
}

TEST_F(Flatland2Test, SampleRectExceedingImageBoundsFailsAtPresent) {
  {
    // 1. Opposite corner (origin + extent) extends past 100x100 bounds: (10, 10, 100, 100).
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);
    const ImageId kImage(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    // Opposite corner (origin + extent) extends past 100x100 bounds: (10, 10, 100, 100).
    layer_props.sample_rect(fuchsia_math::RectF{10.f, 10.f, 100.f, 100.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

    // Neither SetLayerProperties nor SetLayerImage logs an error at call time.
    EXPECT_FALSE(error_log.has_value());

    // Error is caught at Present() time!
    Present(flatland, false);
    EXPECT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("exceeds bound image extent"), std::string::npos);
  }

  {
    // 2. Negative origin extends before (0, 0): (-10, 0, 50, 50).
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);
    const ImageId kImage(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    // Negative origin extends before (0, 0): (-10, 0, 50, 50).
    layer_props.sample_rect(fuchsia_math::RectF{-10.f, 0.f, 50.f, 50.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

    // Neither SetLayerProperties nor SetLayerImage logs an error at call time.
    EXPECT_FALSE(error_log.has_value());

    // Error is caught at Present() time!
    Present(flatland, false);
    EXPECT_TRUE(error_log.has_value());
    EXPECT_NE(error_log->find("exceeds bound image extent"), std::string::npos);
  }
}

TEST_F(Flatland2Test, SampleRectOrderIndependenceSucceeds) {
  {
    // Order 1: SetLayerProperties (sample_rect) then SetLayerImage.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);
    const ImageId kImage(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    layer_props.sample_rect(fuchsia_math::RectF{10.f, 10.f, 80.f, 80.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

    EXPECT_FALSE(error_log.has_value());
    Present(flatland, true);
  }

  {
    // Order 2: SetLayerImage then SetLayerProperties (sample_rect).
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);
    const ImageId kImage(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    layer_props.sample_rect(fuchsia_math::RectF{10.f, 10.f, 80.f, 80.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    EXPECT_FALSE(error_log.has_value());
    Present(flatland, true);
  }
}

TEST_F(Flatland2Test, SampleRectExceedingBoundsIgnoredWhenModeNotImage) {
  {
    // Mode is SOLID_COLOR: sample_rect exceeding image bounds is ignored at Present.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);
    auto allocator = CreateAllocator();

    auto ref_pair = BufferCollectionImportExportTokens::New();
    RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

    EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
        .WillOnce(ReturnPromise(fpromise::ok()));

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);
    const ImageId kImage(4);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                     .size(fuchsia_math::wire::SizeU{100, 100})
                     .Build();
    flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    layer_props.sample_rect(fuchsia_math::RectF{0.f, 0.f, 500.f, 500.f});
    layer_props.color(fuchsia_ui_composition::ColorRgba{1.f, 0.f, 0.f, 1.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kSolidColor);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);

    EXPECT_FALSE(error_log.has_value());
    Present(flatland, true);
  }

  {
    // Mode is IMAGE but no image bound: sample_rect is ignored at Present.
    std::optional<std::string> error_log;
    auto flatland = CreateFlatland2(&error_log);

    const TransformId kRoot(1);
    const LayerId kLayer(2);
    const LayerStackId kStack(3);

    flatland->CreateTransform(kRoot);
    flatland->SetRootTransform(kRoot);
    flatland->CreateLayer(kLayer);
    flatland->CreateLayerStack(kStack);
    flatland->SetStackLayers(kStack, {kLayer});
    flatland->SetTransformContent(kRoot, kStack);

    fidl::Arena arena;
    fuchsia_ui_composition::LayerProperties layer_props;
    layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
    layer_props.sample_rect(fuchsia_math::RectF{0.f, 0.f, 500.f, 500.f});
    layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

    EXPECT_FALSE(error_log.has_value());
    Present(flatland, true);
  }
}

TEST_F(Flatland2Test, BindingInNonImageModeSucceedsAndSticky) {
  std::optional<std::string> error_log;
  auto flatland = CreateFlatland2(&error_log);
  auto allocator = CreateAllocator();

  auto ref_pair = BufferCollectionImportExportTokens::New();
  RegisterBufferCollection(allocator, std::move(ref_pair.export_token), CreateToken(), true);

  EXPECT_CALL(*mock_buffer_collection_importer_, ImportBufferImage(_, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  const TransformId kRoot(1);
  const LayerId kLayer(2);
  const LayerStackId kStack(3);
  const ImageId kImage(4);

  flatland->CreateTransform(kRoot);
  flatland->SetRootTransform(kRoot);
  flatland->CreateLayer(kLayer);
  flatland->CreateLayerStack(kStack);
  flatland->SetStackLayers(kStack, {kLayer});
  flatland->SetTransformContent(kRoot, kStack);

  fidl::Arena arena;
  auto props = fuchsia_ui_composition::wire::ImageProperties::Builder(arena)
                   .size(fuchsia_math::wire::SizeU{100, 100})
                   .Build();
  flatland->CreateImage2(kImage, ToWire(ref_pair.DuplicateImportToken()), 0, props);

  // Layer is INVISIBLE mode by default.
  fuchsia_ui_composition::LayerProperties layer_props;
  layer_props.display_rect(fuchsia_math::RectU{0, 0, 100, 100});
  layer_props.composition_mode(fuchsia_ui_composition::CompositionMode::kInvisible);
  flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(layer_props)));

  // Binding on INVISIBLE layer is completely legal and succeeds.
  flatland->SetLayerImage(kLayer, kImage, std::nullopt, std::nullopt);
  EXPECT_FALSE(error_log.has_value());
  Present(flatland, true);

  auto global_id = flatland->GetGlobalImageIdForTest(kImage);
  EXPECT_EQ(flatland->GetImageObjectForTest(global_id)->ref_count, 2u);

  // Switch to IMAGE mode -> image displays.
  {
    fuchsia_ui_composition::LayerProperties p;
    p.composition_mode(fuchsia_ui_composition::CompositionMode::kImage);
    flatland->SetLayerProperties(kLayer, fidl::ToWire(arena, std::move(p)));
  }
  Present(flatland, true);

  auto renderables = GetRenderables(flatland.get(), 100, 100);
  ASSERT_EQ(renderables.size(), 1u);
  ASSERT_TRUE(std::holds_alternative<ResolvedLayer::ImageContent>(renderables[0].content));
  EXPECT_EQ(std::get<ResolvedLayer::ImageContent>(renderables[0].content).image_id, global_id);
}

// These tests exercise the legacy bridging logic where Flatland1 mutator calls
// are converted into Flatland2 UberStructLayer properties. They will be removed
// when the Flatland1 FIDL API is finally deleted.
class Flatland1FacadeTest : public FlatlandTest {};

// Flatland1FacadeTest.CreateImagePopulatesLayerStackSchema
TEST_F(Flatland1FacadeTest, CreateImagePopulatesLayerStackSchema) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);

  // Present
  Present(flatland, true);

  // Inspect UberStruct
  auto session_id = flatland->GetSessionId();
  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  const auto maybe_handle = flatland->GetContentHandle(kImageId);
  ASSERT_TRUE(maybe_handle.has_value());
  auto content_handle = maybe_handle.value();

  ASSERT_EQ(uber_struct->layer_stacks.size(), 1u);
  auto stack_it = uber_struct->layer_stacks.find(content_handle);
  ASSERT_NE(stack_it, uber_struct->layer_stacks.end());
  ASSERT_EQ(stack_it->second.size(), 1u);

  ASSERT_EQ(uber_struct->layers.size(), 1u);
  auto layer_it = uber_struct->layers.find(stack_it->second[0]);
  ASSERT_NE(layer_it, uber_struct->layers.end());

  const auto& layer = layer_it->second;
  EXPECT_EQ(layer.common, (UberStructLayer::CommonProperties{
                              .display_rect = {{0, 0, 100, 200}},
                              .opacity = 1.f,
                              .blend_mode = types::BlendMode::kReplace(),
                          }));

  ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(layer.content));
  const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);
  EXPECT_NE(content.image_id, allocation::kInvalidImageId);
  EXPECT_EQ(content, (UberStructLayer::ImageModeProperties{
                         .sample_rect = {{0.f, 0.f, 100.f, 200.f}},
                         .transform = types::RotateFlip::kIdentity(),
                         .image_id = content.image_id,
                         .image_width = 100u,
                         .image_height = 200u,
                     }));
}

// Flatland1FacadeTest.SampleRegionResolvesIntoSnapshot
TEST_F(Flatland1FacadeTest, SampleRegionResolvesIntoSnapshot) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);

  fuchsia_math::RectF sample_region = {10.f, 20.f, 30.f, 40.f};
  flatland->SetImageSampleRegion(kImageId, types::RectangleF::From(sample_region));

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kImageId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);

  EXPECT_TRUE(content.sample_rect == types::RectangleF({10.f, 20.f, 30.f, 40.f}));
}

// Flatland1FacadeTest.DestinationSizeResolvesIntoSnapshot
TEST_F(Flatland1FacadeTest, DestinationSizeResolvesIntoSnapshot) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);

  fuchsia_math::wire::SizeU destination_size = {300, 400};
  flatland->SetImageDestinationSize(kImageId, destination_size);

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kImageId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);

  EXPECT_TRUE(layer.common.display_rect == types::Rectangle({0, 0, 300, 400}));
}

// Flatland1FacadeTest.OpacityBlendFlipResolveIntoSnapshot
TEST_F(Flatland1FacadeTest, OpacityBlendFlipResolveIntoSnapshot) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);

  flatland->SetImageOpacity(kImageId, 0.5f);
  flatland->SetImageBlendMode(kImageId, types::BlendMode::kReplace());
  flatland->SetImageFlip(kImageId, fuchsia_ui_composition::ImageFlip::kLeftRight);

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kImageId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  EXPECT_EQ(layer.common.opacity, 0.5f);
  EXPECT_EQ(layer.common.blend_mode, types::BlendMode::kReplace());
  const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);
  EXPECT_EQ(content.transform, types::RotateFlip::kReflectY());
}

// Flatland1FacadeTest.ClampIfNearMatchesLegacy
TEST_F(Flatland1FacadeTest, ClampIfNearMatchesLegacy) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);

  // Set sample region inside epsilon of 0 and 1, but keep x,y >= 0 since Flatland only clamps
  // right/bottom edges
  const float kEpsilon = 0.0001f;
  fuchsia_math::RectF sample_region = {0.f, 0.f, 100.f + kEpsilon / 2.f, 200.f + kEpsilon / 2.f};
  flatland->SetImageSampleRegion(kImageId, types::RectangleF::From(sample_region));

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kImageId).value();

  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);
  // It should be clamped to 0,0,100,200
  EXPECT_TRUE(content.sample_rect == types::RectangleF({0.f, 0.f, 100.f, 200.f}));
}

// Flatland1FacadeTest.ReleaseImageKeepsDisplayingWhileAttached
TEST_F(Flatland1FacadeTest, ReleaseImageKeepsDisplayingWhileAttached) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));
  flatland->SetContent(kRootId, kImageId);
  Present(flatland, true);

  auto content_handle = flatland->GetContentHandle(kImageId).value();

  // Release the image. It is still attached to the transform hierarchy, so it should still display.
  flatland->ReleaseImage(kImageId);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  // It should still be in the snapshot
  EXPECT_TRUE(uber_struct->layer_stacks.contains(content_handle));

  // Detach it from the hierarchy
  flatland->SetContent(kRootId, ContentId(0));
  Present(flatland, true);

  snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  // It should no longer be in the snapshot
  EXPECT_FALSE(uber_struct->layer_stacks.contains(content_handle));
}

// Flatland1FacadeTest.SetContentSwapsStacks
TEST_F(Flatland1FacadeTest, SetContentSwapsStacks) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const ContentId kImageId1{2};
  const ContentId kImageId2{3};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties1, properties2;
  properties1.size(SizeU{100, 200});
  properties2.size(SizeU{150, 250});
  auto ref_pair1 = BufferCollectionImportExportTokens::New();
  auto ref_pair2 = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId1, std::move(ref_pair1),
              std::move(properties1));
  CreateImage(flatland.get(), allocator.get(), kImageId2, std::move(ref_pair2),
              std::move(properties2));

  auto content_handle1 = flatland->GetContentHandle(kImageId1).value();
  auto content_handle2 = flatland->GetContentHandle(kImageId2).value();

  // Attach Image 1
  flatland->SetContent(kRootId, kImageId1);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  EXPECT_TRUE(uber_struct->layer_stacks.contains(content_handle1));
  EXPECT_FALSE(uber_struct->layer_stacks.contains(content_handle2));

  // Attach Image 2
  flatland->SetContent(kRootId, kImageId2);
  Present(flatland, true);

  snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  EXPECT_FALSE(uber_struct->layer_stacks.contains(content_handle1));
  EXPECT_TRUE(uber_struct->layer_stacks.contains(content_handle2));
}

// Flatland1FacadeTest.MultiAttachSameContentId
TEST_F(Flatland1FacadeTest, MultiAttachSameContentId) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const TransformId kChild1Id{2};
  const TransformId kChild2Id{3};
  const ContentId kImageId{4};

  flatland->CreateTransform(kRootId);
  flatland->CreateTransform(kChild1Id);
  flatland->CreateTransform(kChild2Id);
  flatland->AddChild(kRootId, kChild1Id);
  flatland->AddChild(kRootId, kChild2Id);
  flatland->SetRootTransform(kRootId);

  ImageProperties properties;
  properties.size(SizeU{100, 200});
  auto ref_pair = BufferCollectionImportExportTokens::New();
  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              std::move(properties));

  // Attach the same content to two transforms
  flatland->SetContent(kChild1Id, kImageId);
  flatland->SetContent(kChild2Id, kImageId);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kImageId).value();

  // The layer stack should exist in the snapshot map exactly once
  ASSERT_EQ(uber_struct->layer_stacks.size(), 1u);
  ASSERT_TRUE(uber_struct->layer_stacks.contains(content_handle));

  // The layers map should contain exactly one layer
  ASSERT_EQ(uber_struct->layers.size(), 1u);
}

// Flatland1FacadeTest.FilledRectPopulatesSolidColorSnapshot
TEST_F(Flatland1FacadeTest, FilledRectPopulatesSolidColorSnapshot) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);

  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  flatland->CreateFilledRect(kRectId);
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.4f},
                         fuchsia_math::wire::SizeU{100, 200});
  flatland->SetContent(kRootId, kRectId);

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  const auto maybe_handle = flatland->GetContentHandle(kRectId);
  ASSERT_TRUE(maybe_handle.has_value());
  auto content_handle = maybe_handle.value();

  auto stack_iter = uber_struct->layer_stacks.find(content_handle);
  ASSERT_NE(stack_iter, uber_struct->layer_stacks.end());
  ASSERT_EQ(stack_iter->second.size(), 1u);
  auto layer_handle = stack_iter->second[0];

  auto layer_iter = uber_struct->layers.find(layer_handle);
  ASSERT_NE(layer_iter, uber_struct->layers.end());

  // alpha == 0.4 results in `kPremultipliedAlpha` blend mode.
  EXPECT_EQ(
      layer_iter->second,
      (UberStructLayer{
          .content = UberStructLayer::SolidColorModeProperties{.color = {0.1f, 0.2f, 0.3f, 0.4f}},
          .common = {.display_rect = {{0, 0, 100, 200}},
                     .opacity = 1.f,
                     .blend_mode = types::BlendMode::kPremultipliedAlpha()},
      }));
}

// Flatland1FacadeTest.FilledRectBeforeSetSolidFill
TEST_F(Flatland1FacadeTest, FilledRectBeforeSetSolidFill) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);

  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  flatland->CreateFilledRect(kRectId);
  flatland->SetContent(kRootId, kRectId);

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  const auto maybe_handle = flatland->GetContentHandle(kRectId);
  ASSERT_TRUE(maybe_handle.has_value());
  auto content_handle = maybe_handle.value();

  auto stack_iter = uber_struct->layer_stacks.find(content_handle);
  ASSERT_NE(stack_iter, uber_struct->layer_stacks.end());
  ASSERT_EQ(stack_iter->second.size(), 1u);
  auto layer_handle = stack_iter->second[0];

  auto layer_iter = uber_struct->layers.find(layer_handle);
  ASSERT_NE(layer_iter, uber_struct->layers.end());

  // Default solid color has alpha == 1, which results in `kReplace` blend mode.
  EXPECT_EQ(layer_iter->second,
            (UberStructLayer{
                .content = UberStructLayer::SolidColorModeProperties{.color = {1.f, 1.f, 1.f, 1.f}},
                .common = {.display_rect = {{0, 0, 0, 0}},
                           .opacity = 1.f,
                           .blend_mode = types::BlendMode::kReplace()},
            }));
}

// Flatland1FacadeTest.FilledRectInterleavesWithImagesInZOrder
TEST_F(Flatland1FacadeTest, FilledRectInterleavesWithImagesInZOrder) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);
  auto allocator = CreateAllocator();

  const TransformId kRootId{1};
  const TransformId kChild1Id{2};
  const TransformId kChild2Id{3};
  const TransformId kChild3Id{4};
  const ContentId kImage1Id{5};
  const ContentId kRectId{6};
  const ContentId kImage2Id{7};

  flatland->CreateTransform(kRootId);
  flatland->CreateTransform(kChild1Id);
  flatland->CreateTransform(kChild2Id);
  flatland->CreateTransform(kChild3Id);

  flatland->SetRootTransform(kRootId);
  flatland->AddChild(kRootId, kChild1Id);
  flatland->AddChild(kRootId, kChild2Id);
  flatland->AddChild(kRootId, kChild3Id);

  // Child 1 has an image.
  {
    ImageProperties properties;
    properties.size(SizeU{100, 200});
    auto ref_pair = BufferCollectionImportExportTokens::New();
    CreateImage(flatland.get(), allocator.get(), kImage1Id, std::move(ref_pair),
                std::move(properties));
  }
  flatland->SetContent(kChild1Id, kImage1Id);

  // Child 2 has a filled rect.
  flatland->CreateFilledRect(kRectId);
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.5f, 0.5f, 0.5f, 0.5f},
                         fuchsia_math::wire::SizeU{300, 400});
  flatland->SetContent(kChild2Id, kRectId);

  // Child 3 has the other image.
  {
    ImageProperties properties;
    properties.size(SizeU{300, 400});
    auto ref_pair = BufferCollectionImportExportTokens::New();
    CreateImage(flatland.get(), allocator.get(), kImage2Id, std::move(ref_pair),
                std::move(properties));
  }
  flatland->SetContent(kChild3Id, kImage2Id);

  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;

  EXPECT_EQ(uber_struct->layer_stacks.size(), 3u);

  // Verify Child 1 has the image layer
  {
    const auto maybe_image_handle = flatland->GetContentHandle(kImage1Id);
    ASSERT_TRUE(maybe_image_handle.has_value());
    TransformHandle image_handle = maybe_image_handle.value();

    ASSERT_TRUE(uber_struct->layer_stacks.contains(image_handle));
    auto& stack_layers = uber_struct->layer_stacks.find(image_handle)->second;
    ASSERT_EQ(stack_layers.size(), 1u);
    LayerHandle layer_handle = stack_layers[0];
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const UberStructLayer& layer = uber_struct->layers.find(layer_handle)->second;
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(layer.content));
    const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);
    EXPECT_EQ(content, (UberStructLayer::ImageModeProperties{
                           .sample_rect = {{0.f, 0.f, 100.f, 200.f}},
                           .transform = types::RotateFlip::kIdentity(),
                           .image_id = content.image_id,
                           .image_width = 100u,
                           .image_height = 200u,
                       }));
  }

  // Verify Child 2 has the solid color layer
  {
    const auto maybe_rect_handle = flatland->GetContentHandle(kRectId);
    ASSERT_TRUE(maybe_rect_handle.has_value());
    TransformHandle rect_handle = maybe_rect_handle.value();

    ASSERT_TRUE(uber_struct->layer_stacks.contains(rect_handle));
    auto& stack_layers = uber_struct->layer_stacks.find(rect_handle)->second;
    ASSERT_EQ(stack_layers.size(), 1u);
    LayerHandle layer_handle = stack_layers[0];
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    EXPECT_EQ(
        uber_struct->layers.find(layer_handle)->second,
        (UberStructLayer{
            .content = UberStructLayer::SolidColorModeProperties{.color = {0.5f, 0.5f, 0.5f, 0.5f}},
            .common = {.display_rect = {{0, 0, 300, 400}},
                       .opacity = 1.f,
                       .blend_mode = types::BlendMode::kPremultipliedAlpha()},
        }));
  }

  // Verify Child 3 has the image layer
  {
    const auto maybe_image_handle = flatland->GetContentHandle(kImage2Id);
    ASSERT_TRUE(maybe_image_handle.has_value());
    TransformHandle image_handle = maybe_image_handle.value();

    ASSERT_TRUE(uber_struct->layer_stacks.contains(image_handle));
    auto& stack_layers = uber_struct->layer_stacks.find(image_handle)->second;
    ASSERT_EQ(stack_layers.size(), 1u);
    LayerHandle layer_handle = stack_layers[0];
    ASSERT_TRUE(uber_struct->layers.contains(layer_handle));
    const UberStructLayer& layer = uber_struct->layers.find(layer_handle)->second;
    ASSERT_TRUE(std::holds_alternative<UberStructLayer::ImageModeProperties>(layer.content));
    const auto& content = std::get<UberStructLayer::ImageModeProperties>(layer.content);
    EXPECT_EQ(content, (UberStructLayer::ImageModeProperties{
                           .sample_rect = {{0.f, 0.f, 300.f, 400.f}},
                           .transform = types::RotateFlip::kIdentity(),
                           .image_id = content.image_id,
                           .image_width = 300u,
                           .image_height = 400u,
                       }));
  }

  // Verify that they appear in the topology in the correct Z-order.
  TransformHandle root_handle = flatland->GetTransformHandle(kRootId).value();
  TransformHandle child1_handle = flatland->GetTransformHandle(kChild1Id).value();
  TransformHandle child2_handle = flatland->GetTransformHandle(kChild2Id).value();
  TransformHandle child3_handle = flatland->GetTransformHandle(kChild3Id).value();
  TransformHandle image1_handle = flatland->GetContentHandle(kImage1Id).value();
  TransformHandle rect_handle = flatland->GetContentHandle(kRectId).value();
  TransformHandle image2_handle = flatland->GetContentHandle(kImage2Id).value();
  // When `SetRootTransform()` is called, it isn't the real root.  It is attached to the real root.
  ASSERT_EQ(uber_struct->local_topology.size(), 8u);
  EXPECT_EQ(uber_struct->local_topology[0].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[1].handle, root_handle);
  EXPECT_EQ(uber_struct->local_topology[1].child_count, 3u);
  // Children are in depth-first order.
  // Image 1 attached to child 1.
  EXPECT_EQ(uber_struct->local_topology[2].handle, child1_handle);
  EXPECT_EQ(uber_struct->local_topology[2].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[3].handle, image1_handle);
  EXPECT_EQ(uber_struct->local_topology[3].child_count, 0u);
  // Rect attached to child 2.
  EXPECT_EQ(uber_struct->local_topology[4].handle, child2_handle);
  EXPECT_EQ(uber_struct->local_topology[4].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[5].handle, rect_handle);
  EXPECT_EQ(uber_struct->local_topology[5].child_count, 0u);
  // Image2 attached to child 3.
  EXPECT_EQ(uber_struct->local_topology[6].handle, child3_handle);
  EXPECT_EQ(uber_struct->local_topology[6].child_count, 1u);
  EXPECT_EQ(uber_struct->local_topology[7].handle, image2_handle);
  EXPECT_EQ(uber_struct->local_topology[7].child_count, 0u);
}

// Flatland1FacadeTest.ReleaseFilledRectKeepAlive
TEST_F(Flatland1FacadeTest, ReleaseFilledRectKeepAlive) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);

  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  flatland->CreateFilledRect(kRectId);
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.4f},
                         fuchsia_math::wire::SizeU{100, 200});
  flatland->SetContent(kRootId, kRectId);

  Present(flatland, true);

  const auto maybe_handle = flatland->GetContentHandle(kRectId);
  ASSERT_TRUE(maybe_handle.has_value());
  auto content_handle = maybe_handle.value();

  // Release the rect.
  flatland->ReleaseFilledRect(kRectId);

  // Present again. The rect should still be visible because it is still in the active topology.
  Present(flatland, true);

  {
    auto snapshot = uber_struct_system_->Snapshot();
    ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
    auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
    auto stack_iter = uber_struct->layer_stacks.find(content_handle);
    ASSERT_NE(stack_iter, uber_struct->layer_stacks.end());
    ASSERT_EQ(stack_iter->second.size(), 1u);
  }

  // Clear the content of the root transform.
  flatland->SetContent(kRootId, kInvalidContentId);

  // Present again. The layer stack should be cleared and the layer garbage collected.
  Present(flatland, true);

  {
    auto snapshot = uber_struct_system_->Snapshot();
    ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
    auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
    EXPECT_FALSE(uber_struct->layer_stacks.contains(content_handle));
  }
}

// Flatland1FacadeTest.TranslucentFillResultsInPremultiplied
TEST_F(Flatland1FacadeTest, TranslucentFillResultsInPremultiplied) {
  FlatlandConfig config;
  auto flatland = CreateFlatland(config);

  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);

  flatland->CreateFilledRect(kRectId);
  flatland->SetContent(kRootId, kRectId);
  const TransformHandle content_handle = flatland->GetContentHandle(kRectId).value();

  // 1. Translucent fill -> blend_mode == kPremultipliedAlpha
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.5f},
                         fuchsia_math::wire::SizeU{100, 200});
  Present(flatland, true);

  {
    auto snapshot = uber_struct_system_->Snapshot();
    ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
    auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
    auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
    EXPECT_EQ(
        uber_struct->layers.find(layer_handle)->second,
        (UberStructLayer{
            .content = UberStructLayer::SolidColorModeProperties{.color = {0.1f, 0.2f, 0.3f, 0.5f}},
            .common = {.display_rect = {{0, 0, 100, 200}},
                       .opacity = 1.f,
                       .blend_mode = types::BlendMode::kPremultipliedAlpha()},
        }));
  }

  // 2. Re-fill opaque -> blend_mode == kReplace
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 1.f},
                         fuchsia_math::wire::SizeU{100, 200});
  Present(flatland, true);

  {
    auto snapshot = uber_struct_system_->Snapshot();
    ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
    auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
    auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
    EXPECT_EQ(
        uber_struct->layers.find(layer_handle)->second,
        (UberStructLayer{
            .content = UberStructLayer::SolidColorModeProperties{.color = {0.1f, 0.2f, 0.3f, 1.f}},
            .common = {.display_rect = {{0, 0, 100, 200}},
                       .opacity = 1.f,
                       .blend_mode = types::BlendMode::kReplace()},
        }));
  }
}

// The session snapshots the blend mode the client set, including STRAIGHT_ALPHA on a
// solid fill.  Normalization happens downstream, at emission, in
// `ComputeGlobalResolvedLayers()`.
TEST_F(FlatlandTest, StraightAlphaSolidLeftUnchanged) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);
  flatland->CreateFilledRect(kRectId);
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.5f},
                         fuchsia_math::wire::SizeU{100, 200});
  flatland->SetImageBlendMode(kRectId, BlendMode::kStraightAlpha());
  flatland->SetContent(kRootId, kRectId);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kRectId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  EXPECT_EQ(layer.common.blend_mode, types::BlendMode::kStraightAlpha());
}

// When `SetSolidFill()` is called after `SetImageBlendMode()`, it replaces the explicitly-set
// blend mode with whatever is derived from the solid fill color, in this case PREMULTIPLIED_ALPHA
// because alpha < 1.
TEST_F(FlatlandTest, SolidFillRederivesBlendMode) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);
  flatland->CreateFilledRect(kRectId);
  flatland->SetImageBlendMode(kRectId, BlendMode::kReplace());
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.5f},
                         fuchsia_math::wire::SizeU{100, 200});
  flatland->SetContent(kRootId, kRectId);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kRectId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  EXPECT_EQ(layer.common.blend_mode, types::BlendMode::kPremultipliedAlpha());
}

// `SetSolidFill()` derives PREMULTIPLIED_ALPHA blend mode due to content alpha < 1,
// but this is overridden by the REPLACE specified by `SetImageBlendMode()`.
TEST_F(FlatlandTest, SolidFillThenBlendModeOverrides) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  const TransformId kRootId{1};
  const ContentId kRectId{2};
  flatland->CreateTransform(kRootId);
  flatland->SetRootTransform(kRootId);
  flatland->CreateFilledRect(kRectId);
  flatland->SetSolidFill(kRectId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.5f},
                         fuchsia_math::wire::SizeU{100, 200});
  flatland->SetImageBlendMode(kRectId, BlendMode::kReplace());
  flatland->SetContent(kRootId, kRectId);
  Present(flatland, true);

  auto snapshot = uber_struct_system_->Snapshot();
  ASSERT_TRUE(snapshot.map.contains(flatland->GetSessionId()));
  auto uber_struct = snapshot.map.find(flatland->GetSessionId())->second;
  auto content_handle = flatland->GetContentHandle(kRectId).value();
  auto layer_handle = uber_struct->layer_stacks.find(content_handle)->second[0];
  const auto& layer = uber_struct->layers.find(layer_handle)->second;
  EXPECT_EQ(layer.common.blend_mode, types::BlendMode::kReplace());
}

TEST_F(FlatlandTest, ImageMutatorsRejectSolidContent) {
  const ContentId kRectId{1};

  // SetImageFlip rejects solid content.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kRectId);
    flatland->SetImageFlip(kRectId, fuchsia_ui_composition::ImageFlip::kUpDown);
    Present(flatland, false);
  }

  // SetImageSampleRegion rejects solid content.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kRectId);
    flatland->SetImageSampleRegion(kRectId, types::RectangleF({0, 0, 10, 10}));
    Present(flatland, false);
  }

  // SetImageDestinationSize rejects solid content.
  {
    std::shared_ptr<Flatland> flatland = CreateFlatland();
    flatland->CreateFilledRect(kRectId);
    flatland->SetImageDestinationSize(kRectId, {10, 10});
    Present(flatland, false);
  }
}

TEST_F(FlatlandTest, SetSolidFillOnImageRejected) {
  std::shared_ptr<Flatland> flatland = CreateFlatland();
  std::shared_ptr<Allocator> allocator = CreateAllocator();

  const ContentId kImageId{1};
  const uint32_t kImageWidth = 50;
  const uint32_t kImageHeight = 100;
  auto ref_pair = allocation::cpp::BufferCollectionImportExportTokens::New();

  CreateImage(flatland.get(), allocator.get(), kImageId, std::move(ref_pair),
              fuchsia_ui_composition::ImageProperties{{.size = SizeU{kImageWidth, kImageHeight}}});

  flatland->SetSolidFill(kImageId, fuchsia_ui_composition::wire::ColorRgba{0.1f, 0.2f, 0.3f, 0.5f},
                         fuchsia_math::wire::SizeU{100, 200});

  Present(flatland, false);
}

}  // namespace flatland::test
