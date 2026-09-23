// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/input/mouse_source.h"
#include "src/ui/scenic/lib/input/mouse_system.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

// These tests exercise the full mouse delivery flow of InputSystem for
// clients of the fuchsia.ui.pointer.MouseSource protocol.

namespace input::test {

using fup_MouseEvent = fuchsia_ui_pointer::MouseEvent;
using MouseViewStatus = fuchsia_ui_pointer::MouseViewStatus;

using scenic_impl::input::InternalMouseEvent;
using scenic_impl::input::MouseSource;
using scenic_impl::input::StreamId;

constexpr zx_koid_t kContextKoid = 100u;
constexpr zx_koid_t kClient1Koid = 1u;
constexpr zx_koid_t kClient2Koid = 2u;
constexpr zx_koid_t kClient3Koid = 3u;

constexpr StreamId kStream1Id = 11u;
constexpr StreamId kStream2Id = 22u;

constexpr uint32_t kButtonId = 33u;

namespace {

InternalMouseEvent MouseEventTemplate(zx_koid_t target, bool button_down = false) {
  InternalMouseEvent event;
  event.timestamp = 0;
  event.device_id = 1u;
  event.context = kContextKoid;
  event.target = target;
  event.position_in_viewport = glm::vec2(5, 5);  // Middle of viewport.
  event.buttons = {
      .identifiers = {kButtonId},
  };

  if (button_down) {
    event.buttons.pressed = {kButtonId};
  }

  event.viewport.extents.min = {0, 0};
  event.viewport.extents.max = {10, 10};
  return event;
}

}  // namespace

class MouseTest : public gtest::TestLoopFixture {
 public:
  MouseTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        hit_tester_(inspect_node_),
        mouse_system_(hit_tester_,
                      /*request_focus*/ [](auto...) {}) {}

  void SetUp() override {
    ::testing::Test::SetUp();
    zx::result endpoints1 = fidl::CreateEndpoints<fuchsia_ui_pointer::MouseSource>();
    ASSERT_TRUE(endpoints1.is_ok());
    zx::result endpoints2 = fidl::CreateEndpoints<fuchsia_ui_pointer::MouseSource>();
    ASSERT_TRUE(endpoints2.is_ok());

    client1_ptr_.Bind(std::move(endpoints1->client), dispatcher(), &event_handler1_);
    client2_ptr_.Bind(std::move(endpoints2->client), dispatcher(), &event_handler2_);

    OnNewViewTreeSnapshot(NewSnapshot(
        /*hits*/ {}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
    mouse_system_.RegisterMouseSource(std::move(endpoints1->server), kClient1Koid);
    mouse_system_.RegisterMouseSource(std::move(endpoints2->server), kClient2Koid);
  }

  void OnNewViewTreeSnapshot(std::shared_ptr<const view_tree::Snapshot> snapshot) {
    current_snapshot_ = std::move(snapshot);
  }

  // Starts a recursive MouseSource::Watch() loop that collects all received events into
  // |out_events|.
  void StartWatchLoop(fidl::Client<fuchsia_ui_pointer::MouseSource>& mouse_source,
                      std::vector<fup_MouseEvent>& out_events) {
    mouse_source->Watch().Then([this, &mouse_source, &out_events](
                                   fidl::Result<fuchsia_ui_pointer::MouseSource::Watch>& result) {
      if (!result.is_ok()) {
        return;
      }
      auto events = std::move(result->events());
      std::move(events.begin(), events.end(), std::back_inserter(out_events));
      StartWatchLoop(mouse_source, out_events);
    });
  }

 protected:
  // Creates a new snapshot with a hit test that returns |hits|, and a ViewTree with a straight
  // hierarchy matching |hierarchy|.
  std::shared_ptr<view_tree::Snapshot> NewSnapshot(std::vector<zx_koid_t> hits,
                                                   std::vector<zx_koid_t> hierarchy) {
    auto snapshot = std::make_shared<view_tree::Snapshot>();
    snapshot->sequence_number = next_sequence_number_++;

    if (!hierarchy.empty()) {
      snapshot->root = hierarchy[0];
      const auto [_, success] = snapshot->view_tree.try_emplace(hierarchy[0]);
      FX_DCHECK(success);
      if (hierarchy.size() > 1) {
        snapshot->view_tree[hierarchy[0]].children = {hierarchy[1]};
        for (size_t i = 1; i < hierarchy.size() - 1; ++i) {
          snapshot->view_tree[hierarchy[i]].parent = hierarchy[i - 1];
          snapshot->view_tree[hierarchy[i]].children = {hierarchy[i + 1]};
        }
        snapshot->view_tree[hierarchy.back()].parent = hierarchy[hierarchy.size() - 2];
      }
    }

    snapshot->hit_testers.emplace_back(
        [hits](auto...) { return view_tree::SubtreeHitTestResult{.hits = hits}; });

    return snapshot;
  }

  class EventHandler1 : public fidl::AsyncEventHandler<fuchsia_ui_pointer::MouseSource> {
   public:
    explicit EventHandler1(MouseTest* test) : test_(test) {}
    void on_fidl_error(fidl::UnbindInfo info) override {
      if (test_->client1_error_handler_) {
        test_->client1_error_handler_();
      } else {
        FAIL() << "Client1's channel closed";
      }
    }

   private:
    MouseTest* test_;
  };

  class EventHandler2 : public fidl::AsyncEventHandler<fuchsia_ui_pointer::MouseSource> {
   public:
    explicit EventHandler2(MouseTest* test) : test_(test) {}
    void on_fidl_error(fidl::UnbindInfo info) override {
      if (test_->client2_error_handler_) {
        test_->client2_error_handler_();
      } else {
        FAIL() << "Client2's channel closed";
      }
    }

   private:
    MouseTest* test_;
  };

  fit::function<void()> client1_error_handler_;
  fit::function<void()> client2_error_handler_;
  EventHandler1 event_handler1_{this};
  EventHandler2 event_handler2_{this};

 private:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  // Must be initialized before |mouse_system_|.
  sys::testing::ComponentContextProvider context_provider_;
  inspect::Node inspect_node_;
  scenic_impl::input::HitTester hit_tester_;

 protected:
  scenic_impl::input::MouseSystem mouse_system_;
  std::shared_ptr<const view_tree::Snapshot> current_snapshot_;
  fidl::Client<fuchsia_ui_pointer::MouseSource> client1_ptr_;
  fidl::Client<fuchsia_ui_pointer::MouseSource> client2_ptr_;

 private:
  uint64_t next_sequence_number_ = 1;
};

TEST_F(MouseTest, Watch_WithNoInjectedEvents_ShouldNeverReturn) {
  bool callback_triggered = false;
  client1_ptr_->Watch().Then(
      [&callback_triggered](auto& result) { callback_triggered = result.is_ok(); });

  RunLoopUntilIdle();
  EXPECT_FALSE(callback_triggered);
}

TEST_F(MouseTest, IllegalOperation_ShouldCloseChannel) {
  bool channel_closed = false;
  client1_error_handler_ = [&channel_closed] { channel_closed = true; };

  // Illegal operation: calling Watch() twice without getting an event.
  bool callback_triggered = false;
  client1_ptr_->Watch().Then(
      [&callback_triggered](auto& result) { callback_triggered = result.is_ok(); });
  client1_ptr_->Watch().Then(
      [&callback_triggered](auto& result) { callback_triggered = result.is_ok(); });
  RunLoopUntilIdle();
  EXPECT_TRUE(channel_closed);
  EXPECT_FALSE(callback_triggered);
}

TEST_F(MouseTest, ExclusiveInjection_ShouldBeDeliveredOnlyToTarget) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  mouse_system_.InjectMouseEventExclusive(MouseEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_TRUE(received_events2.empty());

  received_events1.clear();
  mouse_system_.InjectMouseEventExclusive(MouseEventTemplate(kClient2Koid), kStream2Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events2.size(), 1u);
  EXPECT_TRUE(received_events1.empty());
}

TEST_F(MouseTest, HitTestedInjection_WithButtonUp_ShouldBeDeliveredOnlyToTopHit) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid, kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  ASSERT_EQ(received_events1.size(), 1u);
  ASSERT_TRUE(received_events1[0].stream_info().has_value());
  EXPECT_EQ(received_events1[0].stream_info()->status(), MouseViewStatus::kEntered);
  EXPECT_TRUE(received_events2.empty());

  // Client 2 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClient2Koid, kClient1Koid},
                                    /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient2Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  {  // Client 1 gets an exit event, but no pointer sample.
    ASSERT_EQ(received_events1.size(), 2u);
    const auto& event = received_events1[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
  {  // Client 2 gets an enter event and a pointer sample.
    ASSERT_EQ(received_events2.size(), 1u);
    const auto& event = received_events2[0];
    EXPECT_TRUE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
  }
}

TEST_F(MouseTest, HitTestedInjection_WithButtonDown_ShouldLatchToTopHit_AndOnlyDeliverToLatched) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid, kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Mouse button down.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_TRUE(received_events2.empty());

  // Remove client 1 from the hit test.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Button still down. Still delivered to client 1.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 2u);
  EXPECT_TRUE(received_events2.empty());

  // Button up again. Client 1 gets a "View exited" event and client 2 gets its first hover event.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  {  // Client 1 gets an exit event, but not a pointer sample.
    ASSERT_EQ(received_events1.size(), 3u);
    const auto& event = received_events1[2];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
  {  // Client 2 gets an enter event and a pointer sample.
    ASSERT_EQ(received_events2.size(), 1u);
    const auto& event = received_events2[0];
    EXPECT_TRUE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
  }
}

TEST_F(MouseTest, LatchedClient_WhenNotInViewTree_ShouldReceiveViewExit) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Client 2 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Mouse button down. Latch on client 2.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_EQ(received_events2.size(), 1u);

  // Remove client 2 from the hit test and the ViewTree
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid}));

  // Button still down, but client 2 gets a ViewExit event and no more pointer samples.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  {  // Client 2 gets an exit event but no pointer sample.
    ASSERT_EQ(received_events2.size(), 2u);
    const auto& event = received_events2[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }

  // Button up. Client 1 gets its first hover event.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_EQ(received_events2.size(), 2u);

  // Client 2 returns.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // And correctly gets another hover event.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events2.size(), 3u);
}

TEST_F(MouseTest, Streams_ShouldLatchIndependently) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid, kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Mouse button down Stream 1. Should latch to client 1.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_TRUE(received_events2.empty());

  // Client 2 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Mouse button down Stream 2. Should latch to client 2.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream2Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_EQ(received_events2.size(), 1u);

  // Stream 1 should continue going to client 1.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 2u);
  EXPECT_EQ(received_events2.size(), 1u);

  // Stream 2 should continue going to client 2.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream2Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 2u);
  EXPECT_EQ(received_events2.size(), 2u);
}

TEST_F(MouseTest, EmptyHitTest_ShouldDeliverToNoOne) {
  std::vector<fup_MouseEvent> received_events;
  StartWatchLoop(client1_ptr_, received_events);

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid}));
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  // Client 1 receives events.
  ASSERT_EQ(received_events.size(), 1u);
  ASSERT_TRUE(received_events[0].stream_info().has_value());
  EXPECT_EQ(received_events[0].stream_info()->status(), MouseViewStatus::kEntered);

  // Hit test returns empty.
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {}, /*hierarchy*/ {kContextKoid, kClient1Koid}));

  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  {  // Client 1 gets an exit event, but no pointer sample.
    ASSERT_EQ(received_events.size(), 2u);
    const auto& event = received_events[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
  received_events.clear();

  // Next injections returns nothing.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid), kStream1Id,
                                          *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events.empty());

  // Button down returns nothing.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events.empty());

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid}));

  // Button up. Client 1 should now receive a hover event.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events.size(), 1u);
}

TEST_F(MouseTest, CancelMouseStream_ShouldSendEvent_OnlyWhenThereIsOngoingStream) {
  std::vector<fup_MouseEvent> received_events1;
  StartWatchLoop(client1_ptr_, received_events1);
  std::vector<fup_MouseEvent> received_events2;
  StartWatchLoop(client2_ptr_, received_events2);

  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Client 1 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient1Koid, kClient2Koid},
      /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Mouse button down Stream 1. Should latch to client 1.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream1Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_TRUE(received_events2.empty());

  // Client 2 is top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {kClient2Koid}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));

  // Hover on stream 2. Should send to client 2
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/true),
                                          kStream2Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 1u);
  EXPECT_EQ(received_events2.size(), 1u);

  // Cancelling stream 1 should deliver view exited event to client 1.
  mouse_system_.CancelMouseStream(kStream1Id);
  RunLoopUntilIdle();
  {
    ASSERT_EQ(received_events1.size(), 2u);
    const auto& event = received_events1[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
  EXPECT_EQ(received_events2.size(), 1u);

  // Cancelling stream 2 should deliver view exited event to client 2.
  mouse_system_.CancelMouseStream(kStream2Id);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events1.size(), 2u);
  {
    ASSERT_EQ(received_events2.size(), 2u);
    const auto& event = received_events2[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }

  received_events1.clear();
  received_events2.clear();

  // More cancel events should be no-ops.
  mouse_system_.CancelMouseStream(kStream1Id);
  mouse_system_.CancelMouseStream(kStream2Id);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events1.empty());
  EXPECT_TRUE(received_events2.empty());

  // Hover on stream 2. Should send to client 2
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream2Id, *current_snapshot_);
  // No top hit.
  OnNewViewTreeSnapshot(NewSnapshot(
      /*hits*/ {}, /*hierarchy*/ {kContextKoid, kClient1Koid, kClient2Koid}));
  // Client 2 gets a view exited event on the next one.
  mouse_system_.InjectMouseEventHitTested(MouseEventTemplate(kClient1Koid, /*button_down=*/false),
                                          kStream2Id, *current_snapshot_);
  RunLoopUntilIdle();
  EXPECT_EQ(received_events2.size(), 2u);
  received_events2.clear();

  // Cancelling stream now should be a no-op.
  mouse_system_.CancelMouseStream(kStream2Id);
  RunLoopUntilIdle();
  EXPECT_TRUE(received_events2.empty());
}

}  // namespace input::test
