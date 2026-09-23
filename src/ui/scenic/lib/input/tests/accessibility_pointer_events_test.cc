// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.input.accessibility/cpp/fidl.h>
#include <fidl/fuchsia.ui.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/sys/cpp/testing/component_context_provider.h>

#include <limits>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/input/input_system.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace input::test {

// These tests check that fuchsia.ui.input.accessibility.PointerEventListener integrates
// correctly with InputSystem. We inject events into the system, accessibility receives them and
// decides whether to consumer or reject them. If consumed the other client should win the contest,
// if rejected the other client should lose.

using AccessibilityPointerEvent = fuchsia_ui_input_accessibility::PointerEvent;
using EventHandling = fuchsia_ui_input_accessibility::EventHandling;
using impl_Phase = scenic_impl::input::Phase;
using fui_Phase = fuchsia_ui_input::PointerEventPhase;
using scenic_impl::input::StreamId;

using TouchInteractionStatus = fuchsia_ui_pointer::TouchInteractionStatus;
using scenic_impl::input::InternalTouchEvent;

constexpr float kNdcEpsilon = std::numeric_limits<float>::epsilon();

constexpr zx_koid_t kContextKoid = 100u;
constexpr zx_koid_t kClientKoid = 111u;
constexpr zx_koid_t kClient2Koid = 222u;

constexpr StreamId kStream1Id = 11u;
constexpr StreamId kStream2Id = 22u;
constexpr StreamId kStream3Id = 33u;

namespace {

InternalTouchEvent PointerEventTemplate(zx_koid_t target, float x, float y, impl_Phase phase) {
  InternalTouchEvent event;
  event.timestamp = 0;
  event.device_id = 1u;
  event.pointer_id = 1u;
  event.phase = phase;
  event.context = kContextKoid;
  event.target = target;
  event.position_in_viewport = glm::vec2(x, y);
  event.buttons = 0;

  event.viewport.extents.min = {0, 0};
  event.viewport.extents.max = {5, 5};

  return event;
}

class MockAccessibilityPointerEventListener
    : public fidl::Server<fuchsia_ui_input_accessibility::PointerEventListener> {
 public:
  MockAccessibilityPointerEventListener(async_dispatcher_t* dispatcher,
                                        scenic_impl::input::TouchSystem* touch_system)
      : is_registered_(std::make_shared<bool>(false)) {
    auto endpoints =
        fidl::Endpoints<fuchsia_ui_input_accessibility::PointerEventListener>::Create();
    binding_ = fidl::BindServer(
        dispatcher, std::move(endpoints.server), this,
        [is_registered = is_registered_](
            MockAccessibilityPointerEventListener*, fidl::UnbindInfo,
            fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventListener>) {
          *is_registered = false;
        });
    touch_system->RegisterA11yListener(
        std::move(endpoints.client),
        [is_registered = is_registered_](bool success) { *is_registered = success; });
  }

  ~MockAccessibilityPointerEventListener() override {
    if (binding_.has_value()) {
      binding_->Unbind();
    }
  }

  bool is_registered() const { return *is_registered_; }
  std::vector<fuchsia_ui_input_accessibility::PointerEvent>& events() { return events_; }

  // Configures how this mock will answer to incoming events.
  //
  // |responses| is a vector, where each pair contains the number of events that
  // will be seen before it responds with an EventHandling value.
  void SetResponses(
      std::vector<std::pair<uint32_t, fuchsia_ui_input_accessibility::EventHandling>> responses) {
    responses_ = std::move(responses);
  }

 private:
  // |fidl::Server<fuchsia_ui_input_accessibility::PointerEventListener>|
  // Performs a response, and resets for the next response.
  void OnEvent(OnEventRequest& request, OnEventCompleter::Sync& completer) override {
    events_.emplace_back(std::move(request.pointer_event()));
    ++num_events_until_response_;
    if (!responses_.empty() && num_events_until_response_ == responses_.front().first) {
      num_events_until_response_ = 0;
      if (binding_.has_value()) {
        auto result = fidl::SendEvent(*binding_)->OnStreamHandled(
            {/*device_id=*/1, /*pointer_id=*/1, /*handled=*/responses_.front().second});
        EXPECT_TRUE(result.is_ok());
      }
      responses_.erase(responses_.begin());
    }
  }

  std::optional<fidl::ServerBindingRef<fuchsia_ui_input_accessibility::PointerEventListener>>
      binding_;
  std::shared_ptr<bool> is_registered_;
  // See |SetResponses|.
  std::vector<std::pair<uint32_t, fuchsia_ui_input_accessibility::EventHandling>> responses_;

  std::vector<fuchsia_ui_input_accessibility::PointerEvent> events_;
  uint32_t num_events_until_response_ = 0;
};

}  // namespace

// Test fixture that sets up a 5x5 "display" and has utilities to wire up views with view refs for
// Accessibility.
class AccessibilityPointerEventsTest : public gtest::TestLoopFixture {
 public:
  AccessibilityPointerEventsTest()
      : dispatcher_setter_(dispatcher(), dispatcher()),
        hit_tester_(inspect_node_),
        touch_system_(dispatcher(), hit_tester_, inspect_node_) {}

  void SetUp() override {
    ::testing::Test::SetUp();
    OnNewViewTreeSnapshot(NewSnapshot(
        /*hits*/ {kClientKoid}, /*hierarchy*/ {kContextKoid, kClientKoid}));

    auto endpoints = fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();
    client_ptr_.Bind(std::move(endpoints.client), dispatcher());
    touch_system_.RegisterTouchSource(std::move(endpoints.server), kClientKoid);
    Watch({});
  }

  void OnNewViewTreeSnapshot(std::shared_ptr<const view_tree::Snapshot> snapshot) {
    current_snapshot_ = std::move(snapshot);
  }

 private:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_;
  // Must be initialized before |touch_system_|.
  sys::testing::ComponentContextProvider context_provider_;
  inspect::Node inspect_node_;
  scenic_impl::input::HitTester hit_tester_;

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

    snapshot->hit_testers.emplace_back([hits = std::move(hits)](auto...) mutable {
      return view_tree::SubtreeHitTestResult{.hits = hits};
    });

    return snapshot;
  }

  scenic_impl::input::TouchSystem touch_system_;
  std::shared_ptr<const view_tree::Snapshot> current_snapshot_;
  std::unordered_map<StreamId, TouchInteractionStatus> client_contests_;

 private:
  // Collects touch events delivered to |client_ptr_| (ignores other events).
  void Watch(std::vector<fuchsia_ui_pointer::TouchResponse> responses) {
    client_ptr_->Watch({{.responses = std::move(responses)}})
        .Then([this](fidl::Result<fuchsia_ui_pointer::TouchSource::Watch>& result) {
          if (!result.is_ok()) {
            return;
          }
          std::vector<fuchsia_ui_pointer::TouchResponse> responses;
          for (auto& event : result->events()) {
            if (event.pointer_sample().has_value()) {
              fuchsia_ui_pointer::TouchResponse response;
              response.response_type(fuchsia_ui_pointer::TouchResponseType::kYes);
              responses.emplace_back(std::move(response));
            } else {
              responses.emplace_back();
            }

            if (event.interaction_result().has_value()) {
              client_contests_[event.interaction_result()->interaction().interaction_id()] =
                  event.interaction_result()->status();
            }
          }

          Watch(std::move(responses));
        });
  }

  fidl::Client<fuchsia_ui_pointer::TouchSource> client_ptr_;
  uint64_t next_sequence_number_ = 1;
};

// This test makes sure that first to register win is working.
TEST_F(AccessibilityPointerEventsTest, RegistersAccessibilityListenerOnlyOnce) {
  MockAccessibilityPointerEventListener listener_1(dispatcher(), &touch_system_);
  RunLoopUntilIdle();

  EXPECT_TRUE(listener_1.is_registered());

  MockAccessibilityPointerEventListener listener_2(dispatcher(), &touch_system_);
  RunLoopUntilIdle();

  EXPECT_FALSE(listener_2.is_registered()) << "The second listener that attempts to connect should "
                                              "fail, as there is already one connected.";
  EXPECT_TRUE(listener_1.is_registered()) << "First listener should still be connected.";
}

// Registration is first-come, first-served, but a slot held by a listener whose channel has closed
// must be released: otherwise a restarted accessibility manager could never register again.
TEST_F(AccessibilityPointerEventsTest, RegistersNewListenerAfterDisconnection) {
  {
    MockAccessibilityPointerEventListener listener_1(dispatcher(), &touch_system_);
    RunLoopUntilIdle();
    EXPECT_TRUE(listener_1.is_registered());
    // Let the first listener go out of scope, which closes its channel.
  }
  RunLoopUntilIdle();

  MockAccessibilityPointerEventListener listener_2(dispatcher(), &touch_system_);
  RunLoopUntilIdle();
  EXPECT_TRUE(listener_2.is_registered())
      << "A listener should be able to register after the previous one disconnected.";
}

// In this test two pointer event streams will be injected in the input system. The first one, with
// four pointer events, will be accepted in the second pointer event. The second one, also with four
// pointer events, will be accepted in the fourth one.
TEST_F(AccessibilityPointerEventsTest, ConsumesPointerEvents) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);
  listener.SetResponses({
      {3, EventHandling::kConsumed},
      {6, EventHandling::kConsumed},
  });

  // A touch sequence that starts at the (2.5,2.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_EQ(client_contests_.count(kStream1Id), 0u)
      << "Contest should not end until Accessibility allows it";

  // Verify accessibility's events.
  ASSERT_EQ(listener.events().size(), 2u);
  {
    const AccessibilityPointerEvent& add = listener.events()[0];
    EXPECT_EQ(add.phase(), fui_Phase::kAdd);
    EXPECT_EQ(add.ndc_point()->x(), 0.f);
    EXPECT_EQ(add.ndc_point()->y(), 0.f);
    EXPECT_EQ(add.viewref_koid(), kClientKoid);
    EXPECT_EQ(add.local_point()->x(), 2.5);
    EXPECT_EQ(add.local_point()->y(), 2.5);
  }
  {
    const AccessibilityPointerEvent& down = listener.events()[1];
    EXPECT_EQ(down.phase(), fui_Phase::kDown);
    EXPECT_EQ(down.ndc_point()->x(), 0.f);
    EXPECT_EQ(down.ndc_point()->y(), 0.f);
    EXPECT_EQ(down.viewref_koid(), kClientKoid);
    EXPECT_EQ(down.local_point()->x(), 2.5);
    EXPECT_EQ(down.local_point()->y(), 2.5);
  }

  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kDenied);
  {
    ASSERT_EQ(listener.events().size(), 3u);
    {
      const AccessibilityPointerEvent& move = listener.events()[2];
      EXPECT_EQ(move.phase(), fui_Phase::kMove);
      EXPECT_EQ(move.ndc_point()->x(), 0.f);
      EXPECT_EQ(move.ndc_point()->y(), 0.f);
      EXPECT_EQ(move.viewref_koid(), kClientKoid);
      EXPECT_EQ(move.local_point()->x(), 2.5);
      EXPECT_EQ(move.local_point()->y(), 2.5);
    }
  }
  listener.events().clear();

  // Accessibility consumed the two events. Continue sending pointer events in the same stream.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 3.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 3.5f, impl_Phase::kRemove), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  // Verify accessibility's events.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 3u);
    {
      const AccessibilityPointerEvent& move = events[0];
      EXPECT_EQ(move.phase(), fui_Phase::kMove);
      EXPECT_EQ(move.ndc_point()->x(), 0.f);
      EXPECT_NEAR(move.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(move.viewref_koid(), kClientKoid);
      EXPECT_EQ(move.local_point()->x(), 2.5);
      EXPECT_EQ(move.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& up = events[1];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_EQ(up.ndc_point()->x(), 0.f);
      EXPECT_NEAR(up.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(up.viewref_koid(), kClientKoid);
      EXPECT_EQ(up.local_point()->x(), 2.5);
      EXPECT_EQ(up.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[2];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_EQ(remove.ndc_point()->x(), 0.f);
      EXPECT_NEAR(remove.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(remove.viewref_koid(), kClientKoid);
      EXPECT_EQ(remove.local_point()->x(), 2.5);
      EXPECT_EQ(remove.local_point()->y(), 3.5);
    }
  }
  listener.events().clear();

  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 3.5f, 1.5f, impl_Phase::kAdd), kStream2Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 3.5f, 1.5f, impl_Phase::kRemove), kStream2Id,
      *current_snapshot_);  // Consume happens here.
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kDenied);

  // Verify accessibility's events.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 4u);
    {
      const AccessibilityPointerEvent& add = events[0];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_NEAR(add.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(add.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 3.5);
      EXPECT_EQ(add.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& down = events[1];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_NEAR(down.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(down.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 3.5);
      EXPECT_EQ(down.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& up = events[2];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_NEAR(up.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(up.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(up.viewref_koid(), kClientKoid);
      EXPECT_EQ(up.local_point()->x(), 3.5);
      EXPECT_EQ(up.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[3];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_NEAR(remove.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(remove.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(remove.viewref_koid(), kClientKoid);
      EXPECT_EQ(remove.local_point()->x(), 3.5);
      EXPECT_EQ(remove.local_point()->y(), 1.5);
    }
  }
}

// One pointer stream is injected in the input system. The listener rejects the pointer event. this
// test makes sure that buffered (past), as well as future pointer events are sent to the view.
TEST_F(AccessibilityPointerEventsTest, RejectsPointerEvents) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);
  listener.SetResponses({{1, EventHandling::kRejected}});

  // A touch sequence that starts at the (2.5,2.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kGranted);

  // Verify accessibility's events. Note that the listener must see two events here, but not later,
  // because it rejects the stream in the second pointer event.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 2u);
    {
      const AccessibilityPointerEvent& add = events[0];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_EQ(add.ndc_point()->x(), 0.f);
      EXPECT_EQ(add.ndc_point()->y(), 0.f);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 2.5);
      EXPECT_EQ(add.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& down = events[1];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_EQ(down.ndc_point()->x(), 0.f);
      EXPECT_EQ(down.ndc_point()->y(), 0.f);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 2.5);
      EXPECT_EQ(down.local_point()->y(), 2.5);
    }
  }

  listener.events().clear();

  // Send the rest of the stream.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 3.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 3.5f, impl_Phase::kRemove), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  EXPECT_TRUE(listener.events().empty())
      << "Accessibility should stop receiving events in a stream after rejecting it.";
}

// In this test three streams will be injected in the input system, where the first will be
// consumed, the second rejected and the third also consumed.
TEST_F(AccessibilityPointerEventsTest, AlternatingResponses) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);
  listener.SetResponses({
      {2, EventHandling::kConsumed},
      {2, EventHandling::kRejected},
      {2, EventHandling::kConsumed},
  });

  // Send in the input.
  // First stream:
  // A touch sequence that starts at the (1.5,1.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kRemove), kStream1Id,
      *current_snapshot_);  // Consume happens here.
  // Second stream:
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream2Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kRemove), kStream2Id,
      *current_snapshot_);  // Reject happens here.
  // Third stream:
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 3.5f, 3.5f, impl_Phase::kAdd), kStream3Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 3.5f, 3.5f, impl_Phase::kRemove), kStream3Id,
      *current_snapshot_);  // Consume happens here.
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kDenied);
  ASSERT_NE(client_contests_.count(kStream2Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream2Id), TouchInteractionStatus::kGranted);
  ASSERT_NE(client_contests_.count(kStream3Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream3Id), TouchInteractionStatus::kDenied);

  // Verify accessibility's events.
  // The listener should see all events, as it is configured to see the entire stream before
  // consuming / rejecting it.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 12u);
    {
      const AccessibilityPointerEvent& add = events[0];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_NEAR(add.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(add.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 1.5);
      EXPECT_EQ(add.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& down = events[1];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_NEAR(down.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(down.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 1.5);
      EXPECT_EQ(down.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& up = events[2];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_NEAR(up.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(up.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(up.viewref_koid(), kClientKoid);
      EXPECT_EQ(up.local_point()->x(), 1.5);
      EXPECT_EQ(up.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[3];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_NEAR(remove.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(remove.ndc_point()->y(), -.4, kNdcEpsilon);
      EXPECT_EQ(remove.viewref_koid(), kClientKoid);
      EXPECT_EQ(remove.local_point()->x(), 1.5);
      EXPECT_EQ(remove.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& add = events[4];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_EQ(add.ndc_point()->x(), 0.f);
      EXPECT_EQ(add.ndc_point()->y(), 0.f);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 2.5);
      EXPECT_EQ(add.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& down = events[5];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_EQ(down.ndc_point()->x(), 0.f);
      EXPECT_EQ(down.ndc_point()->y(), 0.f);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 2.5);
      EXPECT_EQ(down.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& up = events[6];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_EQ(up.ndc_point()->x(), 0.f);
      EXPECT_EQ(up.ndc_point()->y(), 0.f);
      EXPECT_EQ(up.viewref_koid(), kClientKoid);
      EXPECT_EQ(up.local_point()->x(), 2.5);
      EXPECT_EQ(up.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[7];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_EQ(remove.ndc_point()->x(), 0.f);
      EXPECT_EQ(remove.ndc_point()->y(), 0.f);
      EXPECT_EQ(remove.viewref_koid(), kClientKoid);
      EXPECT_EQ(remove.local_point()->x(), 2.5);
      EXPECT_EQ(remove.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& add = events[8];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_NEAR(add.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(add.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 3.5);
      EXPECT_EQ(add.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& down = events[9];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_NEAR(down.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(down.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 3.5);
      EXPECT_EQ(down.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& up = events[10];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_NEAR(up.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(up.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(up.viewref_koid(), kClientKoid);
      EXPECT_EQ(up.local_point()->x(), 3.5);
      EXPECT_EQ(up.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[11];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_NEAR(remove.ndc_point()->x(), .4, kNdcEpsilon);
      EXPECT_NEAR(remove.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(remove.viewref_koid(), kClientKoid);
      EXPECT_EQ(remove.local_point()->x(), 3.5);
      EXPECT_EQ(remove.local_point()->y(), 3.5);
    }
  }

  // Make sure we didn't disconnect at some point for some reason.
  EXPECT_TRUE(listener.is_registered());
}

// This test makes sure that if there is a stream in progress and the accessibility listener
// connects, the existing stream is not sent to the listener.
TEST_F(AccessibilityPointerEventsTest, DiscardActiveStreamOnConnection) {
  // Send in the input.
  // A touch sequence that starts at the (1.5,1.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kGranted);

  // Now, connect the accessibility listener in the middle of a stream.
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);

  // Sends the rest of the stream.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kRemove), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  EXPECT_TRUE(listener.is_registered());
  EXPECT_TRUE(listener.events().empty()) << "Accessibility should not receive events from a stream "
                                            "already in progress when it was registered.";
}

// This tests makes sure that if there is an active stream, and accessibility disconnects, the
// stream is sent to regular clients.
TEST_F(AccessibilityPointerEventsTest, DispatchEventsAfterDisconnection) {
  {
    MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);

    // Send in the input.
    // A touch sequence that starts at the (2.5,2.5) location of the 5x5 display.
    touch_system_.InjectTouchEventHitTested(
        PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream1Id,
        *current_snapshot_);
    RunLoopUntilIdle();

    ASSERT_EQ(client_contests_.count(kStream1Id), 0u) << "Contest should not have ended";

    // Verify client's accessibility pointer events.
    EXPECT_EQ(listener.events().size(), 2u);

    // Let the accessibility listener go out of scope without answering what we are going to do with
    // the pointer events.
  }

  RunLoopUntilIdle();
  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kGranted);
}

// In this test, there are two views. We inject a pointer event stream onto both. We
// alternate the elevation of the views; in each case, the topmost view's ViewRef KOID should be
// observed.
TEST_F(AccessibilityPointerEventsTest, ExposeTopMostViewRefKoid) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);

  // Set client 1 above client 2 in the hit test.
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClientKoid, kClient2Koid},
                                    /*hierarchy*/ {kContextKoid, kClientKoid, kClient2Koid}));

  // Scene is now set up; send in the input.
  // A touch sequence that starts at the (2.5,2.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  // Verify accessibility's events.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 2u);
    {
      const AccessibilityPointerEvent& add = events[0];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_EQ(add.ndc_point()->x(), 0.f);
      EXPECT_EQ(add.ndc_point()->y(), 0.f);
      EXPECT_EQ(add.viewref_koid(), kClientKoid);
      EXPECT_EQ(add.local_point()->x(), 2.5);
      EXPECT_EQ(add.local_point()->y(), 2.5);
    }
    {
      const AccessibilityPointerEvent& down = events[1];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_EQ(down.ndc_point()->x(), 0.f);
      EXPECT_EQ(down.ndc_point()->y(), 0.f);
      EXPECT_EQ(down.viewref_koid(), kClientKoid);
      EXPECT_EQ(down.local_point()->x(), 2.5);
      EXPECT_EQ(down.local_point()->y(), 2.5);
    }
  }

  // Now set client 2 above client 1 in the hit test.
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {kClient2Koid, kClientKoid},
                                    /*hierarchy*/ {kContextKoid, kClientKoid, kClient2Koid}));
  // Send in the rest of the stream.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 3.5f, impl_Phase::kRemove), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  // Verify accessibility's events.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 4u);
    {
      const AccessibilityPointerEvent& up = events[2];
      EXPECT_EQ(up.phase(), fui_Phase::kUp);
      EXPECT_NEAR(up.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(up.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(up.viewref_koid(), kClient2Koid);
      EXPECT_EQ(up.local_point()->x(), 1.5);
      EXPECT_EQ(up.local_point()->y(), 3.5);
    }
    {
      const AccessibilityPointerEvent& remove = events[3];
      EXPECT_EQ(remove.phase(), fui_Phase::kRemove);
      EXPECT_NEAR(remove.ndc_point()->x(), -.4, kNdcEpsilon);
      EXPECT_NEAR(remove.ndc_point()->y(), .4, kNdcEpsilon);
      EXPECT_EQ(remove.viewref_koid(), kClient2Koid);
      EXPECT_EQ(remove.local_point()->x(), 1.5);
      EXPECT_EQ(remove.local_point()->y(), 3.5);
    }
  }
}

// This test has a ADD event see an empty hit test, which means there is no client that latches.
// However, (1) accessibility should receive initial events, and (2) acceptance by accessibility (on
// first MOVE) means accessibility continues to observe events, despite absence of latch.
TEST_F(AccessibilityPointerEventsTest, NoAddLatchAndA11yAccepts) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);
  // Respond after three events: ADD / MOVE / MOVE.
  listener.SetResponses({{3, EventHandling::kConsumed}});

  // No hits!
  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {}, /*hierarchy*/ {kContextKoid, kClientKoid}));
  // Send in input.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 0.5f, 0.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_EQ(client_contests_.count(kStream1Id), 0u)
      << "Contest should not have ended (nor even begun)";

  // The rest of the input should have hits.
  OnNewViewTreeSnapshot(
      NewSnapshot(/*hits*/ {kClientKoid}, /*hierarchy*/ {kContextKoid, kClientKoid}));
  // A touch sequence that starts at the (1.5,1.5) location of the 5x5 display.
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 1.5f, 1.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  touch_system_.InjectTouchEventHitTested(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kChange), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_EQ(client_contests_.count(kStream1Id), 0u)
      << "Contest should not have ended (nor even begun)";

  // Verify accessibility received all events.
  {
    const std::vector<AccessibilityPointerEvent>& events = listener.events();
    ASSERT_EQ(events.size(), 4u);
    {
      const AccessibilityPointerEvent& add = events[0];
      EXPECT_EQ(add.phase(), fui_Phase::kAdd);
      EXPECT_EQ(add.viewref_koid(), ZX_KOID_INVALID);
    }
    {
      const AccessibilityPointerEvent& down = events[1];
      EXPECT_EQ(down.phase(), fui_Phase::kDown);
      EXPECT_EQ(down.viewref_koid(), ZX_KOID_INVALID);
    }
    {
      const AccessibilityPointerEvent& move = events[2];
      EXPECT_EQ(move.phase(), fui_Phase::kMove);
      EXPECT_EQ(move.viewref_koid(), kClientKoid);
      EXPECT_EQ(move.local_point()->x(), 1.5);
      EXPECT_EQ(move.local_point()->y(), 1.5);
    }
    {
      const AccessibilityPointerEvent& move = events[3];
      EXPECT_EQ(move.phase(), fui_Phase::kMove);
      EXPECT_EQ(move.viewref_koid(), kClientKoid);
      EXPECT_EQ(move.local_point()->x(), 2.5);
      EXPECT_EQ(move.local_point()->y(), 2.5);
    }
  }
}

// Injection in EXCLUSIVE_TARGET mode should never be delivered to a11y. This test sets up
// a valid environment for a11y to get events, except for the DispatchPolicy.
TEST_F(AccessibilityPointerEventsTest, TopHitInjectionByNonRootView_IsNotDeliveredToA11y) {
  MockAccessibilityPointerEventListener listener(dispatcher(), &touch_system_);

  OnNewViewTreeSnapshot(NewSnapshot(/*hits*/ {}, /*hierarchy*/ {kContextKoid, kClientKoid}));
  // A touch sequence that starts at the (2.5,2.5) location of the 5x5 display.
  touch_system_.InjectTouchEventExclusive(
      PointerEventTemplate(kClientKoid, 2.5f, 2.5f, impl_Phase::kAdd), kStream1Id,
      *current_snapshot_);
  RunLoopUntilIdle();

  ASSERT_NE(client_contests_.count(kStream1Id), 0u) << "Contest should have ended";
  EXPECT_EQ(client_contests_.at(kStream1Id), TouchInteractionStatus::kGranted);
  EXPECT_TRUE(listener.events().empty());
}

}  // namespace input::test
