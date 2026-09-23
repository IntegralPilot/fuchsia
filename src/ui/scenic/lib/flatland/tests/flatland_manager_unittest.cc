// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_manager.h"

#include <fidl/fuchsia.scenic.scheduling/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/fit/thread_checker.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/allocation/mock_buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/tests/logging_event_loop.h"
#include "src/ui/scenic/lib/flatland/tests/mock_flatland_presenter.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/scheduling/id.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

using ::testing::_;
using ::testing::AtLeast;
using ::testing::Return;

using flatland::FlatlandManager;
using flatland::FlatlandPresenter;
using flatland::LinkSystem;
using flatland::MockFlatlandPresenter;
using flatland::UberStructSystem;

// These macros works like functions that check a variety of conditions, but if those conditions
// fail, the line number for the failure will appear in-line rather than in a function.

// This macro calls Present() on a Flatland object and immediately triggers the session update
// for all sessions so that changes from that Present() are visible in global systems. This is
// primarily useful for testing the user-facing Flatland API.
//
// This macro must be used within a test using the FlatlandManagerTest harness.
//
// |flatland| is a Flatland object constructed with the MockFlatlandPresenter owned by the
// FlatlandManagerTest harness. |session_id| is the SessionId for |flatland|. |expect_success|
// should be false if the call to Present() is expected to trigger an error.
#define PRESENT(flatland, session_id, expect_success)                                        \
  {                                                                                          \
    const auto num_pending_sessions = GetNumPendingSessionUpdates(session_id);               \
    if (expect_success) {                                                                    \
      EXPECT_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _)); \
    }                                                                                        \
    fuchsia_ui_composition::PresentArgs present_args;                                        \
    present_args.requested_presentation_time(0);                                             \
    present_args.acquire_fences(std::vector<zx::event>{});                                   \
    present_args.release_fences(std::vector<zx::event>{});                                   \
    present_args.present_fences(std::vector<zx::counter>{});                                 \
    present_args.unsquashable(false);                                                        \
    fuchsia_ui_composition::FlatlandPresentRequest request;                                  \
    request.args(std::move(present_args));                                                   \
    EXPECT_TRUE(flatland->Present(std::move(request)).is_ok());                              \
    /* If expecting success, wait for the worker thread to process the request. */           \
    if (expect_success) {                                                                    \
      RunLoopUntil([this, session_id, num_pending_sessions] {                                \
        return GetNumPendingSessionUpdates(session_id) > num_pending_sessions;               \
      });                                                                                    \
    }                                                                                        \
  }

namespace {

struct TestEventHandler : public fidl::AsyncEventHandler<fuchsia_ui_composition::Flatland> {
  fit::function<void(fuchsia_ui_composition::OnNextFrameBeginValues)> on_next_frame_begin;
  fit::function<void(fuchsia_scenic_scheduling::FramePresentedInfo)> on_frame_presented;
  fit::function<void(fuchsia_ui_composition::FlatlandError)> on_error;
  bool is_unbound = false;

  void OnNextFrameBegin(
      fidl::Event<fuchsia_ui_composition::Flatland::OnNextFrameBegin>& event) override {
    if (on_next_frame_begin) {
      on_next_frame_begin(std::move(event.values()));
    }
  }

  void OnFramePresented(
      fidl::Event<fuchsia_ui_composition::Flatland::OnFramePresented>& event) override {
    if (on_frame_presented) {
      on_frame_presented(std::move(event.frame_presented_info()));
    }
  }

  void OnError(fidl::Event<fuchsia_ui_composition::Flatland::OnError>& event) override {
    if (on_error) {
      on_error(event.error());
    }
  }

  void on_fidl_error(fidl::UnbindInfo info) override { is_unbound = true; }
};

struct FlatlandClient {
  std::shared_ptr<TestEventHandler> event_handler;
  fidl::Client<fuchsia_ui_composition::Flatland> client;

  TestEventHandler& events() { return *event_handler; }
  auto operator->() const { return client.operator->(); }
  bool is_bound() const { return client.is_valid() && event_handler && !event_handler->is_unbound; }
  void reset() {
    client = {};
    event_handler.reset();
  }
};

class FlatlandManagerTest : public LoggingEventLoop, public ::testing::Test {
 public:
  FlatlandManagerTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        uber_struct_system_(std::make_shared<UberStructSystem>()),
        link_system_(std::make_shared<LinkSystem>(uber_struct_system_->GetNextInstanceId())) {}

  void SetUp() override {
    ::testing::Test::SetUp();

    mock_flatland_presenter_ = std::make_shared<::testing::StrictMock<MockFlatlandPresenter>>();

    ON_CALL(*mock_flatland_presenter_, ScheduleUpdateForSession(_, _, _, _, _, _, _))
        .WillByDefault(::testing::Invoke(
            [&](zx::time requested_presentation_time, scheduling::SchedulingIdPair id_pair,
                bool unsquashable, std::vector<zx::event> release_fences,
                std::vector<zx::counter> release_counters, std::vector<zx::counter> present_fences,
                bool schedule_asap) {
              EXPECT_TRUE(release_fences.empty());
              EXPECT_TRUE(release_counters.empty());

              // The ID pair must not be already registered.
              EXPECT_FALSE(pending_presents_.contains(id_pair));
              pending_presents_.insert(id_pair);

              // Ensure present IDs are strictly increasing.
              auto& queue = pending_session_updates_[id_pair.session_id];
              EXPECT_TRUE(queue.empty() || queue.back() < id_pair.present_id);

              // Save the pending present ID.
              queue.push(id_pair.present_id);
            }));

    ON_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos())
        .WillByDefault(::testing::Invoke([&]() {
          // The requested_prediction_span should be at least one frame.

          // Give back at least one info.
          std::vector<scheduling::FuturePresentationInfo> presentation_infos;
          auto& info = presentation_infos.emplace_back();
          info.latch_point = zx::time(5);
          info.presentation_time = zx::time(10);

          return presentation_infos;
        }));

    ON_CALL(*mock_flatland_presenter_, RemoveSession(_, _))
        .WillByDefault(::testing::Invoke(
            [&](scheduling::SessionId session_id, std::optional<zx::event> release_fence) {
              async::PostTask(this->dispatcher(), [&, session_id]() {
                std::lock_guard lock(removed_session_thread_checker_);
                removed_sessions_.insert(session_id);
              });
            }));

    const display::WireDisplayId kDisplayId = {.value = 1};
    constexpr uint32_t kDisplayWidth = 640;
    constexpr uint32_t kDisplayHeight = 480;
    constexpr uint32_t kMaxDisplayLayersCount = 2;
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> importers;
    manager_ = std::make_unique<FlatlandManager>(
        dispatcher(), mock_flatland_presenter_, uber_struct_system_, link_system_,
        std::make_shared<display::Display>(kDisplayId, kDisplayWidth, kDisplayHeight,
                                           kMaxDisplayLayersCount),
        importers,
        /*register_view_focuser*/ [this](auto...) { view_focuser_registered_ = true; },
        /*register_view_ref_focused*/ [this](auto...) { view_ref_focused_registered_ = true; },
        /*register_touch_source*/ [this](auto...) { touch_source_registered_ = true; },
        /*register_mouse_source*/ [this](auto...) { mouse_source_registered_ = true; },
        /*register_touch_source_v2*/ [this](auto...) { touch_source_v2_registered_ = true; },
        /*register_mouse_source_v2*/ [this](auto...) { mouse_source_v2_registered_ = true; });
  }

  void TearDown() override {
    // |manager_| may have been reset during the test. If not, run until all sessions have closed,
    // which depends on the worker threads receiving "peer closed" for the clients created in
    // the tests.
    {
      std::lock_guard lock(removed_session_thread_checker_);
      std::for_each(removed_sessions_.begin(), removed_sessions_.end(), [](auto session_id) {
        FX_LOGS(INFO) << "`removed_sessions_` includes " << session_id;
      });
      removed_sessions_.clear();
    }
    if (manager_) {
      const size_t initial_session_count = manager_->GetSessionCount();
      FX_LOGS(INFO) << "initial_session_count=" << initial_session_count;
      EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(_, _))
          .Times(AtLeast(initial_session_count));
      RunLoopUntil([this, initial_session_count] {
        std::lock_guard lock(removed_session_thread_checker_);
        // It could be tempting to only check a single condition here. However,
        // it won't work as expected. `FlatlandManager` posts the task
        // destroying `Flatland` instance on the Session's loop thread, and then
        // deletes the session from its session list immediately.
        //
        // The `Flatland` destructor later calls presenter API to remove the
        // session, which is handled on the presenter handler's FIDL loop.
        // There may be a critical section in between and we should make sure
        // both conditions are fulfilled before proceeding.
        auto current_session_count = manager_->GetSessionCount();
        std::for_each(removed_sessions_.begin(), removed_sessions_.end(), [](auto session_id) {
          FX_LOGS(INFO) << "`removed_sessions_` includes " << session_id;
        });
        FX_LOGS(INFO) << "current_session_count=" << current_session_count;

        // We can't test for equality between `removed_sessions_.size()` and `initial_session_count`
        // due to a race condition: the about-to-be-destroyed session might already have been
        // removed from the manager, but the presenter is not notified until the end of the Flatland
        // destructor, which occurs on a different thread.
        return current_session_count == 0 && removed_sessions_.size() >= initial_session_count;
      });
    }

    auto snapshot = uber_struct_system_->Snapshot();
    EXPECT_TRUE(snapshot.map.empty());

    manager_.reset();
    RunLoopUntilIdle();

    EXPECT_EQ(uber_struct_system_->GetSessionCount(), 0ul);

    pending_presents_.clear();
    pending_session_updates_.clear();
    mock_flatland_presenter_.reset();
    {
      std::lock_guard lock(removed_session_thread_checker_);
      removed_sessions_.clear();
    }

    ::testing::Test::TearDown();
  }

  FlatlandClient CreateFlatland(
      const flatland::FlatlandConfig& config = flatland::FlatlandConfig{}) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_ui_composition::Flatland>::Create();
    const scheduling::SessionId id =
        manager_->CreateFlatland(std::move(server_end), config).value();
    FX_LOGS(INFO) << "Created flatland with ID " << id;
    auto event_handler = std::make_shared<TestEventHandler>();
    fidl::Client<fuchsia_ui_composition::Flatland> client(std::move(client_end), dispatcher(),
                                                          event_handler.get());
    return FlatlandClient{
        .event_handler = std::move(event_handler),
        .client = std::move(client),
    };
  }

  // Returns the number of currently pending session updates for |session_id|.
  size_t GetNumPendingSessionUpdates(scheduling::SessionId session_id) {
    const auto& queue = pending_session_updates_[session_id];
    return queue.size();
  }

  // Returns the next pending PresentId for |session_id| and removes it from the list of pending
  // session updates. Fails if |session_id| has no pending presents.
  scheduling::PresentId PopPendingPresent(scheduling::SessionId session_id) {
    auto& queue = pending_session_updates_[session_id];
    EXPECT_FALSE(queue.empty());

    auto next_present_id = queue.front();
    queue.pop();
    return next_present_id;
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  std::shared_ptr<::testing::StrictMock<MockFlatlandPresenter>> mock_flatland_presenter_;
  const std::shared_ptr<UberStructSystem> uber_struct_system_;

  std::unique_ptr<FlatlandManager> manager_;

  // Storage for |mock_flatland_presenter_|.
  std::set<scheduling::SchedulingIdPair> pending_presents_;
  std::unordered_map<scheduling::SessionId, std::queue<scheduling::PresentId>>
      pending_session_updates_;

  // std::unordered_set is not thread-safe. Here we add the thread checker
  // to make sure that it is only used on the test loop (which runs on the same
  // thread as test main thread).
  fit::thread_checker removed_session_thread_checker_;
  std::unordered_set<scheduling::SessionId> removed_sessions_
      FIT_GUARDED(removed_session_thread_checker_);

  const std::shared_ptr<LinkSystem> link_system_;

  bool view_focuser_registered_ = false;
  bool view_ref_focused_registered_ = false;
  bool touch_source_registered_ = false;
  bool mouse_source_registered_ = false;
  bool touch_source_v2_registered_ = false;
  bool mouse_source_v2_registered_ = false;
};

}  // namespace

namespace flatland::test {

TEST_F(FlatlandManagerTest, CreateFlatlands) {
  FlatlandClient flatland1 = CreateFlatland();
  FlatlandClient flatland2 = CreateFlatland();

  RunLoopUntilIdle();

  EXPECT_TRUE(flatland1.is_bound());
  EXPECT_TRUE(flatland2.is_bound());
  EXPECT_EQ(manager_->GetSessionCount(), 2ul);
}

TEST_F(FlatlandManagerTest, UntrustedFlatlandRunsOnIndependentThread) {
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  RunLoopUntilIdle();

  // The session dispatcher should NOT be the main thread's dispatcher.
  auto session_dispatcher = manager_->GetSessionDispatcherForTest(id);
  ASSERT_NE(session_dispatcher, nullptr);
  EXPECT_NE(session_dispatcher, dispatcher());
}

TEST_F(FlatlandManagerTest, TrustedFlatlandRunsOnMainThread) {
  FlatlandClient flatland = CreateFlatland({.use_trusted_flatland_api = true});
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  RunLoopUntilIdle();

  // The session dispatcher SHOULD be the main thread's dispatcher.
  auto session_dispatcher = manager_->GetSessionDispatcherForTest(id);
  ASSERT_NE(session_dispatcher, nullptr);
  EXPECT_EQ(session_dispatcher, dispatcher());
}

TEST_F(FlatlandManagerTest, CreateViewportedFlatlands) {
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();

  FlatlandClient parent = CreateFlatland();
  const fuchsia_ui_composition::ContentId kLinkId(1);
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::ChildViewWatcher>::Create();
  fuchsia_ui_composition::ViewportProperties properties;
  properties.logical_size(fuchsia_math::SizeU(1, 2));
  fuchsia_ui_composition::FlatlandCreateViewportRequest viewport_request;
  viewport_request.viewport_id(kLinkId);
  viewport_request.token(std::move(parent_token));
  viewport_request.properties(std::move(properties));
  viewport_request.child_view_watcher(std::move(child_view_watcher_server_end));
  EXPECT_TRUE(parent->CreateViewport(std::move(viewport_request)).is_ok());

  {
    FlatlandClient child = CreateFlatland();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
    fuchsia_ui_composition::FlatlandCreateViewRequest view_request;
    view_request.token(std::move(child_token));
    view_request.parent_viewport_watcher(std::move(parent_viewport_watcher_server_end));
    EXPECT_TRUE(child->CreateView(std::move(view_request)).is_ok());

    RunLoopUntilIdle();
    EXPECT_EQ(manager_->GetSessionCount(), 2ul);
    RunLoopUntil([this] { return !link_system_->GetResolvedTopologyLinks().empty(); });

    EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(_, _));
  }

  RunLoopUntil([this] { return link_system_->GetResolvedTopologyLinks().empty(); });
}

TEST_F(FlatlandManagerTest, ClientDiesBeforeManager) {
  scheduling::SessionId id;
  {
    FlatlandClient flatland = CreateFlatland();
    id = uber_struct_system_->GetLatestInstanceId();

    RunLoopUntilIdle();

    EXPECT_TRUE(flatland.is_bound());

    // |flatland| falls out of scope, killing the session.
    EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(id, _));

    // FlatlandManager::RemoveFlatlandInstance() will be posted on main thread and may not be run
    // yet.
    RunLoopUntilIdle();
  }

  // The session should show up in the set of removed sessions.
  RunLoopUntil([this] {
    std::lock_guard lock(removed_session_thread_checker_);
    // It could be tempting to only check a single condition here, however,
    // this will cause a race and won't work as expected, since FlatlandManager
    // removing session from manager's session list and Flatland impl requests
    // presenter to remove the session occur on different threads.
    // See the comment at `TearDown()` for details.
    return manager_->GetSessionCount() == 0 && removed_sessions_.size() == 1;
  });
  {
    std::lock_guard lock(removed_session_thread_checker_);
    EXPECT_TRUE(removed_sessions_.contains(id));
  }
}

TEST_F(FlatlandManagerTest, ManagerDiesBeforeClients) {
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  RunLoopUntilIdle();

  EXPECT_TRUE(flatland.is_bound());
  EXPECT_EQ(manager_->GetSessionCount(), 1ul);

  // Explicitly kill the server.
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(id, _));
  manager_.reset();

  EXPECT_EQ(uber_struct_system_->GetSessionCount(), 0ul);

  RunLoopUntil([this] {
    std::lock_guard lock(removed_session_thread_checker_);
    return removed_sessions_.size() == 1ul;
  });

  {
    std::lock_guard lock(removed_session_thread_checker_);
    EXPECT_TRUE(removed_sessions_.contains(id));
  }

  // Wait until unbound.
  RunLoopUntil([&flatland] { return !flatland.is_bound(); });

  // FlatlandManager::RemoveFlatlandInstance() will be posted on main thread and may not be run yet.
  RunLoopUntilIdle();
}

TEST_F(FlatlandManagerTest, FirstPresentReturnsMaxPresentCredits) {
  // Setup a Flatland instance with an OnNextFrameBegin() callback.
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  uint32_t returned_tokens = 0;
  flatland.events().on_next_frame_begin =
      [&returned_tokens](fuchsia_ui_composition::OnNextFrameBeginValues values) {
        returned_tokens += values.additional_present_credits().value_or(0);
        EXPECT_TRUE(returned_tokens > 0);
        EXPECT_FALSE(values.future_presentation_infos()->empty());
      };

  // Present once, but don't update sessions.
  PRESENT(flatland, id, true);

  auto snapshot = uber_struct_system_->Snapshot();
  EXPECT_TRUE(snapshot.map.empty());

  EXPECT_EQ(GetNumPendingSessionUpdates(id), 1ul);

  // Update the session, this should return max tokens through OnNextFrameBegin().
  const auto next_present_id = PopPendingPresent(id);
  manager_->UpdateInstances({{id, next_present_id}});

  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  snapshot = uber_struct_system_->Snapshot();
  EXPECT_EQ(snapshot.map.size(), 1u);
  EXPECT_TRUE(snapshot.map.contains(id));

  RunLoopUntil([&returned_tokens] { return returned_tokens != 0; });
  EXPECT_EQ(returned_tokens, scheduling::FrameScheduler::kMaxPresentsInFlight);
  EXPECT_EQ(GetNumPendingSessionUpdates(id), 0ul);
}

TEST_F(FlatlandManagerTest, UpdateInstancesReturnsPresentCredits) {
  // Setup two Flatland instances with OnNextFrameBegin() callbacks.
  FlatlandClient flatland1 = CreateFlatland();
  const scheduling::SessionId id1 = uber_struct_system_->GetLatestInstanceId();

  uint32_t returned_tokens1 = 0;
  flatland1.events().on_next_frame_begin =
      [&returned_tokens1](fuchsia_ui_composition::OnNextFrameBeginValues values) {
        returned_tokens1 += values.additional_present_credits().value_or(0);
        EXPECT_TRUE(returned_tokens1 > 0);
        EXPECT_FALSE(values.future_presentation_infos()->empty());
      };

  FlatlandClient flatland2 = CreateFlatland();
  const scheduling::SessionId id2 = uber_struct_system_->GetLatestInstanceId();

  uint32_t returned_tokens2 = 0;
  flatland2.events().on_next_frame_begin =
      [&returned_tokens2](fuchsia_ui_composition::OnNextFrameBeginValues values) {
        returned_tokens2 += values.additional_present_credits().value_or(0);
        EXPECT_TRUE(returned_tokens2 > 0);
        EXPECT_FALSE(values.future_presentation_infos()->empty());
      };

  {  // Go through the initial present so both instances have multiple credits.
    PRESENT(flatland1, id1, true);
    PRESENT(flatland2, id2, true);
    const auto next_present_id1 = PopPendingPresent(id1);
    const auto next_present_id2 = PopPendingPresent(id2);
    manager_->UpdateInstances({{id1, next_present_id1}, {id2, next_present_id2}});
    EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
    manager_->SendHintsToStartRendering();
    RunLoopUntil([&] {
      return returned_tokens1 == scheduling::FrameScheduler::kMaxPresentsInFlight &&
             returned_tokens2 == scheduling::FrameScheduler::kMaxPresentsInFlight;
    });
    EXPECT_EQ(GetNumPendingSessionUpdates(id1), 0ul);
    EXPECT_EQ(GetNumPendingSessionUpdates(id2), 0ul);
    // Now forget about the returned tokens.
    returned_tokens1 = 0;
    returned_tokens2 = 0;
  }

  // Present both instances twice, but don't update sessions.
  PRESENT(flatland1, id1, true);
  PRESENT(flatland1, id1, true);

  PRESENT(flatland2, id2, true);
  PRESENT(flatland2, id2, true);

  EXPECT_EQ(GetNumPendingSessionUpdates(id1), 2ul);
  EXPECT_EQ(GetNumPendingSessionUpdates(id2), 2ul);

  // Update the first session, but only with the first PresentId, which should push an UberStruct
  // and return one token to the first instance.
  auto next_present_id1 = PopPendingPresent(id1);
  manager_->UpdateInstances({{id1, next_present_id1}});

  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  RunLoopUntil([&returned_tokens1] { return returned_tokens1 != 0; });

  EXPECT_EQ(returned_tokens1, 1u);
  EXPECT_EQ(returned_tokens2, 0u);

  EXPECT_EQ(GetNumPendingSessionUpdates(id1), 1ul);
  EXPECT_EQ(GetNumPendingSessionUpdates(id2), 2ul);

  returned_tokens1 = 0;

  // Update only the second session and consume both PresentIds, which should push an UberStruct
  // and return two tokens to the second instance.
  PopPendingPresent(id2);
  const auto next_present_id2 = PopPendingPresent(id2);

  manager_->UpdateInstances({{id2, next_present_id2}});

  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  const auto snapshot = uber_struct_system_->Snapshot();
  EXPECT_EQ(snapshot.map.size(), 2u);
  EXPECT_TRUE(snapshot.map.contains(id1));
  EXPECT_TRUE(snapshot.map.contains(id2));

  RunLoopUntil([&returned_tokens2] { return returned_tokens2 != 0; });

  EXPECT_EQ(returned_tokens1, 0u);
  EXPECT_EQ(returned_tokens2, 2u);

  EXPECT_EQ(GetNumPendingSessionUpdates(id1), 1ul);
  EXPECT_EQ(GetNumPendingSessionUpdates(id2), 0ul);
}

// It is possible for the session to update multiple times in a row before
// SendHintsToStartRendering() is called. If that's the case, we need to ensure that present credits
// returned from the first update are not lost.
TEST_F(FlatlandManagerTest, ConsecutiveUpdateInstances_ReturnsCorrectPresentCredits) {
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  uint32_t returned_tokens = 0;
  flatland.events().on_next_frame_begin =
      [&returned_tokens](fuchsia_ui_composition::OnNextFrameBeginValues values) {
        // `=` rather than the `+=` used elsewhere in this file, on purpose:
        // FlatlandManager batches the credits from consecutive UpdateInstances()
        // calls into one OnNextFrameBegin. With `=`, the EXPECT_EQ at the end of
        // this test fails if the credits arrive as two events; with `+=` it would
        // pass. This is the likely reason the original test assigned instead of
        // accumulating.
        returned_tokens = values.additional_present_credits().value_or(0);
        EXPECT_TRUE(returned_tokens > 0);
        EXPECT_FALSE(values.future_presentation_infos()->empty());
      };

  {  // Receive the initial allotment of tokens, then forget those tokens.
    PRESENT(flatland, id, true);
    const auto next_present_id = PopPendingPresent(id);
    manager_->UpdateInstances({{id, next_present_id}});
    EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
    manager_->SendHintsToStartRendering();
    RunLoopUntil(
        [&] { return returned_tokens == scheduling::FrameScheduler::kMaxPresentsInFlight; });
    EXPECT_EQ(GetNumPendingSessionUpdates(id), 0ul);
    returned_tokens = 0;
  }

  // Present twice, but don't update the session yet.
  PRESENT(flatland, id, true);
  PRESENT(flatland, id, true);
  EXPECT_EQ(GetNumPendingSessionUpdates(id), 2ul);

  // Update the session, but only with the first PresentId, which should push an UberStruct
  // and return one token to the first instance.
  auto next_present_id = PopPendingPresent(id);
  manager_->UpdateInstances({{id, next_present_id}});

  // Update again.
  next_present_id = PopPendingPresent(id);
  manager_->UpdateInstances({{id, next_present_id}});

  // Finally, the work is done according to the frame scheduler.
  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  const auto snapshot = uber_struct_system_->Snapshot();
  EXPECT_EQ(snapshot.map.size(), 1u);
  EXPECT_TRUE(snapshot.map.contains(id));

  RunLoopUntil([&returned_tokens] { return returned_tokens != 0; });

  EXPECT_EQ(returned_tokens, 2u);

  EXPECT_EQ(GetNumPendingSessionUpdates(id), 0ul);
}

TEST_F(FlatlandManagerTest, PresentWithoutTokensClosesSession) {
  // Setup a Flatland instance with an OnNextFrameBegin() callback.
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_ui_composition::FlatlandError> error_returned;
  flatland.events().on_error = [&error_returned](fuchsia_ui_composition::FlatlandError error) {
    error_returned = error;
  };

  // Present until no tokens remain.
  PRESENT(flatland, id, true);
  EXPECT_TRUE(flatland.is_bound());

  // Present one more time and ensure the session is closed.
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(id, _));
  PRESENT(flatland, id, false);

  // The instance will eventually be unbound, but it takes a pair of thread hops to complete since
  // the destroy_instance_function() posts a task from the worker to the main and that task
  // ultimately posts the destruction back onto the worker.
  RunLoopUntil([&flatland] { return !flatland.is_bound(); });
  EXPECT_EQ(error_returned, fuchsia_ui_composition::FlatlandError::kNoPresentsRemaining);

  // Wait until all Flatland threads are destroyed.
  RunLoopUntil([this] { return manager_->GetAliveSessionCount() == 0; });

  // FlatlandManager::RemoveFlatlandInstance() will be posted on main thread and may not be run yet.
  RunLoopUntilIdle();
}

TEST_F(FlatlandManagerTest, ErrorClosesSession) {
  // Setup a Flatland instance with an OnNextFrameBegin() callback.
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_ui_composition::FlatlandError> error_returned;
  flatland.events().on_error = [&error_returned](fuchsia_ui_composition::FlatlandError error) {
    error_returned = error;
  };
  EXPECT_TRUE(flatland.is_bound());

  // Queue a bad SetRootTransform call ensure the session is closed.
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(id, _));
  fuchsia_ui_composition::FlatlandSetRootTransformRequest root_request;
  root_request.transform_id(fuchsia_ui_composition::TransformId(2));
  EXPECT_TRUE(flatland->SetRootTransform(std::move(root_request)).is_ok());
  PRESENT(flatland, id, false);

  // The instance will eventually be unbound, but it takes a pair of thread hops to complete since
  // the destroy_instance_function() posts a task from the worker to the main and that task
  // ultimately posts the destruction back onto the worker.
  RunLoopUntil([&flatland] { return !flatland.is_bound(); });
  EXPECT_EQ(error_returned, fuchsia_ui_composition::FlatlandError::kBadOperation);

  // Wait until all Flatland threads are destroyed.
  RunLoopUntil([this] { return manager_->GetAliveSessionCount() == 0; });

  // FlatlandManager::RemoveFlatlandInstance() will be posted on main thread and may not be run
  // yet.
  RunLoopUntilIdle();
}

TEST_F(FlatlandManagerTest, TokensAreReplenishedAfterRunningOut) {
  // Setup a Flatland instance with an OnNextFrameBegin() callback.
  FlatlandClient flatland = CreateFlatland();
  const scheduling::SessionId id = uber_struct_system_->GetLatestInstanceId();

  uint32_t tokens_remaining = 0;
  flatland.events().on_next_frame_begin =
      [&tokens_remaining](fuchsia_ui_composition::OnNextFrameBeginValues values) {
        tokens_remaining += values.additional_present_credits().value_or(0);
        EXPECT_TRUE(tokens_remaining > 0);
      };

  {  // Receive the initial allotment of tokens.
    PRESENT(flatland, id, true);
    const auto next_present_id = PopPendingPresent(id);
    manager_->UpdateInstances({{id, next_present_id}});
    EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
    manager_->SendHintsToStartRendering();
    RunLoopUntil(
        [&] { return tokens_remaining == scheduling::FrameScheduler::kMaxPresentsInFlight; });
  }

  // Present until no tokens remain.
  while (tokens_remaining > 0) {
    PRESENT(flatland, id, true);
    --tokens_remaining;
  }

  // Process the first present.
  auto next_present_id = PopPendingPresent(id);
  manager_->UpdateInstances({{id, next_present_id}});

  // Signal that the work is done, which should return present credits to the client.
  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  RunLoopUntil([&tokens_remaining] { return tokens_remaining != 0; });

  // Present once more which should succeed.
  PRESENT(flatland, id, true);
  EXPECT_TRUE(flatland.is_bound());
}

TEST_F(FlatlandManagerTest, OnFramePresentedEvent) {
  // Setup two Flatland instances with OnFramePresented() callbacks.
  FlatlandClient flatland1 = CreateFlatland();
  const scheduling::SessionId id1 = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_scenic_scheduling::FramePresentedInfo> info1;
  flatland1.events().on_frame_presented =
      [&info1](fuchsia_scenic_scheduling::FramePresentedInfo info) { info1 = std::move(info); };

  FlatlandClient flatland2 = CreateFlatland();
  const scheduling::SessionId id2 = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_scenic_scheduling::FramePresentedInfo> info2;
  flatland2.events().on_frame_presented =
      [&info2](fuchsia_scenic_scheduling::FramePresentedInfo info) { info2 = std::move(info); };

  {  // Go through the initial present so both instances have multiple credits.
    uint32_t returned_tokens1 = 0;
    flatland1.events().on_next_frame_begin =
        [&returned_tokens1](fuchsia_ui_composition::OnNextFrameBeginValues values) {
          returned_tokens1 += values.additional_present_credits().value_or(0);
        };
    uint32_t returned_tokens2 = 0;
    flatland2.events().on_next_frame_begin =
        [&returned_tokens2](fuchsia_ui_composition::OnNextFrameBeginValues values) {
          returned_tokens2 += values.additional_present_credits().value_or(0);
        };
    PRESENT(flatland1, id1, true);
    PRESENT(flatland2, id2, true);
    const auto next_present_id1 = PopPendingPresent(id1);
    const auto next_present_id2 = PopPendingPresent(id2);
    manager_->UpdateInstances({{id1, next_present_id1}, {id2, next_present_id2}});
    EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
    manager_->SendHintsToStartRendering();
    RunLoopUntil([&] {
      return returned_tokens1 == scheduling::FrameScheduler::kMaxPresentsInFlight &&
             returned_tokens2 == scheduling::FrameScheduler::kMaxPresentsInFlight;
    });
  }

  // Present both instances twice, but don't update sessions.
  PRESENT(flatland1, id1, true);
  PRESENT(flatland1, id1, true);

  PRESENT(flatland2, id2, true);
  PRESENT(flatland2, id2, true);

  // Call OnFramePresented() with a PresentId for the first session and ensure the event fires.
  scheduling::PresentTimestamps timestamps{
      .presented_time = zx::time(111),
      .vsync_interval = zx::duration(11),
  };
  zx::time latch_time1 = zx::time(123);
  auto next_present_id1 = PopPendingPresent(id1);

  std::unordered_map<scheduling::SessionId,
                     std::map<scheduling::PresentId, /*latched_time*/ zx::time>>
      latch_times;
  latch_times[id1] = {{next_present_id1, latch_time1}};

  manager_->OnFramePresented(latch_times, timestamps);

  // Wait until the event has fired.
  RunLoopUntil([&info1] { return info1.has_value(); });

  // Verify that info1 contains the expected data.
  EXPECT_EQ(zx::time(info1->actual_presentation_time()), timestamps.presented_time);
  EXPECT_EQ(info1->num_presents_allowed(), 0ul);
  EXPECT_EQ(info1->presentation_infos().size(), 1ul);
  EXPECT_EQ(zx::time(info1->presentation_infos()[0].latched_time().value()), latch_time1);

  // Run the loop again to show that info2 hasn't been populated.
  RunLoopUntilIdle();
  EXPECT_FALSE(info2.has_value());

  // Call OnFramePresented with all the remaining PresentIds and ensure an event fires for both.
  info1.reset();
  latch_times.clear();

  timestamps = scheduling::PresentTimestamps({
      .presented_time = zx::time(222),
      .vsync_interval = zx::duration(22),
  });
  latch_time1 = zx::time(234);
  auto latch_time2_1 = zx::time(345);
  auto latch_time2_2 = zx::time(456);
  next_present_id1 = PopPendingPresent(id1);
  auto next_present_id2_1 = PopPendingPresent(id2);
  auto next_present_id2_2 = PopPendingPresent(id2);

  latch_times[id1] = {{next_present_id1, latch_time1}};
  latch_times[id2] = {{next_present_id2_1, latch_time2_1}, {next_present_id2_2, latch_time2_2}};

  manager_->OnFramePresented(latch_times, timestamps);

  // Wait until both events have fired.
  RunLoopUntil([&info1] { return info1.has_value(); });
  RunLoopUntil([&info2] { return info2.has_value(); });

  // Verify that both infos contain the expected data.
  EXPECT_EQ(zx::time(info1->actual_presentation_time()), timestamps.presented_time);
  EXPECT_EQ(info1->num_presents_allowed(), 0ul);
  EXPECT_EQ(info1->presentation_infos().size(), 1ul);
  EXPECT_EQ(zx::time(info1->presentation_infos()[0].latched_time().value()), latch_time1);

  EXPECT_EQ(zx::time(info2->actual_presentation_time()), timestamps.presented_time);
  EXPECT_EQ(info2->num_presents_allowed(), 0ul);
  EXPECT_EQ(info2->presentation_infos().size(), 2ul);
  EXPECT_EQ(zx::time(info2->presentation_infos()[0].latched_time().value()), latch_time2_1);
  EXPECT_EQ(zx::time(info2->presentation_infos()[1].latched_time().value()), latch_time2_2);

  // Call `OnFramePresented()` after the first session has terminated.
  //
  // Verify that Scenic does not crash, and that the second session still gets its
  // `OnFramePresented` event.
  //
  // Note: The iteration order of sessions within the argument to `OnFramePresented()`
  // is dependent on a hash function. Hence: if the hash ordering varies from one test
  // run to another (for identical builds), and there is a bug in `OnFramePresented()`,
  // this test could flake.
  PRESENT(flatland1, id1, true);
  PRESENT(flatland2, id2, true);
  EXPECT_CALL(*mock_flatland_presenter_, RemoveSession(id1, _));
  flatland1.reset();
  FX_LOGS(INFO) << "Waiting for removal of session " << id1;
  RunLoopUntil([this, id1] {
    std::lock_guard lock(removed_session_thread_checker_);
    return removed_sessions_.find(id1) != removed_sessions_.end();
  });
  info2.reset();
  manager_->OnFramePresented({{id1, {{PopPendingPresent(id1), zx::time(789)}}},
                              {id2, {{PopPendingPresent(id2), zx::time(789)}}}},
                             scheduling::PresentTimestamps({
                                 .presented_time = zx::time(777),
                                 .vsync_interval = zx::duration(16),
                             }));
  FX_LOGS(INFO) << "Waiting for event on session " << id2;
  RunLoopUntil([&info2] { return info2.has_value(); });
}

TEST_F(FlatlandManagerTest, SkipsOnFramePresentedComparison) {
  // Client 1: skips_on_frame_presented = true
  flatland::FlatlandConfig config1;
  config1.skips_on_frame_presented = true;
  FlatlandClient flatland1 = CreateFlatland(config1);
  const scheduling::SessionId id1 = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_scenic_scheduling::FramePresentedInfo> info1;
  flatland1.events().on_frame_presented =
      [&info1](fuchsia_scenic_scheduling::FramePresentedInfo info) { info1 = std::move(info); };

  // Client 2: skips_on_frame_presented = false
  flatland::FlatlandConfig config2;
  config2.skips_on_frame_presented = false;
  FlatlandClient flatland2 = CreateFlatland(config2);
  const scheduling::SessionId id2 = uber_struct_system_->GetLatestInstanceId();

  std::optional<fuchsia_scenic_scheduling::FramePresentedInfo> info2;
  flatland2.events().on_frame_presented =
      [&info2](fuchsia_scenic_scheduling::FramePresentedInfo info) { info2 = std::move(info); };

  RunLoopUntilIdle();

  // Present both instances.
  PRESENT(flatland1, id1, true);
  PRESENT(flatland2, id2, true);

  // Call OnFramePresented() with PresentIds for both sessions.
  scheduling::PresentTimestamps timestamps{
      .presented_time = zx::time(111),
      .vsync_interval = zx::duration(11),
  };

  auto next_present_id1 = PopPendingPresent(id1);
  auto next_present_id2 = PopPendingPresent(id2);

  std::unordered_map<scheduling::SessionId,
                     std::map<scheduling::PresentId, /*latched_time*/ zx::time>>
      latch_times;
  latch_times[id1] = {{next_present_id1, zx::time(123)}};
  latch_times[id2] = {{next_present_id2, zx::time(456)}};

  manager_->OnFramePresented(latch_times, timestamps);

  // Wait until the event has fired for client 2.
  RunLoopUntil([&info2] { return info2.has_value(); });

  // Verify that client 1 did NOT receive OnFramePresented.
  EXPECT_FALSE(info1.has_value());

  // Verify that client 2 DID receive OnFramePresented.
  EXPECT_TRUE(info2.has_value());
}

TEST_F(FlatlandManagerTest, SkipsPresentCreditsComparison) {
  // Client 1: skips_present_credits = true
  flatland::FlatlandConfig config1;
  config1.skips_present_credits = true;
  FlatlandClient flatland1 = CreateFlatland(config1);
  const scheduling::SessionId id1 = uber_struct_system_->GetLatestInstanceId();

  bool began1 = false;
  flatland1.events().on_next_frame_begin =
      [&began1](fuchsia_ui_composition::OnNextFrameBeginValues values) { began1 = true; };

  // Client 2: skips_present_credits = false
  flatland::FlatlandConfig config2;
  config2.skips_present_credits = false;
  FlatlandClient flatland2 = CreateFlatland(config2);
  const scheduling::SessionId id2 = uber_struct_system_->GetLatestInstanceId();

  bool began2 = false;
  flatland2.events().on_next_frame_begin =
      [&began2](fuchsia_ui_composition::OnNextFrameBeginValues values) { began2 = true; };

  RunLoopUntilIdle();

  // Present both instances.
  PRESENT(flatland1, id1, true);
  PRESENT(flatland2, id2, true);

  // Update instances to trigger credits return.
  auto next_present_id1 = PopPendingPresent(id1);
  auto next_present_id2 = PopPendingPresent(id2);
  manager_->UpdateInstances({{id1, next_present_id1}, {id2, next_present_id2}});

  EXPECT_CALL(*mock_flatland_presenter_, GetFuturePresentationInfos());
  manager_->SendHintsToStartRendering();

  // Wait until the event has fired for client 2.
  RunLoopUntil([&began2] { return began2; });

  // Verify that client 1 did NOT receive OnNextFrameBegin.
  EXPECT_FALSE(began1);

  // Verify that client 2 DID receive OnNextFrameBegin.
  EXPECT_TRUE(began2);
}

TEST_F(FlatlandManagerTest, ViewBoundProtocolsAreRegistered) {
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  FlatlandClient parent = CreateFlatland();
  const fuchsia_ui_composition::ContentId kLinkId(1);
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::ChildViewWatcher>::Create();
  fuchsia_ui_composition::ViewportProperties properties;
  properties.logical_size(fuchsia_math::SizeU(1, 2));
  fuchsia_ui_composition::FlatlandCreateViewportRequest viewport_request;
  viewport_request.viewport_id(kLinkId);
  viewport_request.token(std::move(parent_token));
  viewport_request.properties(std::move(properties));
  viewport_request.child_view_watcher(std::move(child_view_watcher_server_end));
  EXPECT_TRUE(parent->CreateViewport(std::move(viewport_request)).is_ok());

  FlatlandClient child = CreateFlatland();

  auto [view_focuser_client_end, view_focuser_server_end] =
      fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  auto [view_ref_focused_client_end, view_ref_focused_server_end] =
      fidl::Endpoints<fuchsia_ui_views::ViewRefFocused>::Create();
  auto [touch_source_client_end, touch_source_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();
  auto [mouse_source_client_end, mouse_source_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::MouseSource>::Create();

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
  fuchsia_ui_composition::ViewBoundProtocols protocols;
  protocols.view_focuser(std::move(view_focuser_server_end));
  protocols.view_ref_focused(std::move(view_ref_focused_server_end));
  protocols.touch_source(std::move(touch_source_server_end));
  protocols.mouse_source(std::move(mouse_source_server_end));
  fuchsia_ui_composition::FlatlandCreateView2Request view_request;
  view_request.token(std::move(child_token));
  view_request.view_identity(scenic::cpp::NewViewIdentityOnCreation());
  view_request.protocols(std::move(protocols));
  view_request.parent_viewport_watcher(std::move(parent_viewport_watcher_server_end));
  EXPECT_TRUE(child->CreateView2(std::move(view_request)).is_ok());

  RunLoopUntil([this] {
    return view_focuser_registered_ && view_ref_focused_registered_ && touch_source_registered_ &&
           mouse_source_registered_;
  });
}

TEST_F(FlatlandManagerTest, ViewBoundProtocolsV2AreRegistered) {
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  FlatlandClient parent = CreateFlatland();
  const fuchsia_ui_composition::ContentId kLinkId(1);
  auto [child_view_watcher_client_end, child_view_watcher_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::ChildViewWatcher>::Create();
  fuchsia_ui_composition::ViewportProperties properties;
  properties.logical_size(fuchsia_math::SizeU(1, 2));
  fuchsia_ui_composition::FlatlandCreateViewportRequest viewport_request;
  viewport_request.viewport_id(kLinkId);
  viewport_request.token(std::move(parent_token));
  viewport_request.properties(std::move(properties));
  viewport_request.child_view_watcher(std::move(child_view_watcher_server_end));
  EXPECT_TRUE(parent->CreateViewport(std::move(viewport_request)).is_ok());

  FlatlandClient child = CreateFlatland();

  auto [view_focuser_client_end, view_focuser_server_end] =
      fidl::Endpoints<fuchsia_ui_views::Focuser>::Create();
  auto [view_ref_focused_client_end, view_ref_focused_server_end] =
      fidl::Endpoints<fuchsia_ui_views::ViewRefFocused>::Create();
  auto [touch_source_v2_client_end, touch_source_v2_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::TouchSourceV2>::Create();
  auto [mouse_source_v2_client_end, mouse_source_v2_server_end] =
      fidl::Endpoints<fuchsia_ui_pointer::MouseSourceV2>::Create();

  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
  fuchsia_ui_composition::ViewBoundProtocols protocols;
  protocols.view_focuser(std::move(view_focuser_server_end));
  protocols.view_ref_focused(std::move(view_ref_focused_server_end));
  protocols.touch_source_v2(std::move(touch_source_v2_server_end));
  protocols.mouse_source_v2(std::move(mouse_source_v2_server_end));
  fuchsia_ui_composition::FlatlandCreateView2Request view_request;
  view_request.token(std::move(child_token));
  view_request.view_identity(scenic::cpp::NewViewIdentityOnCreation());
  view_request.protocols(std::move(protocols));
  view_request.parent_viewport_watcher(std::move(parent_viewport_watcher_server_end));
  EXPECT_TRUE(child->CreateView2(std::move(view_request)).is_ok());

  RunLoopUntil([this] {
    return view_focuser_registered_ && view_ref_focused_registered_ &&
           touch_source_v2_registered_ && mouse_source_v2_registered_;
  });
}

#undef PRESENT

}  // namespace flatland::test
