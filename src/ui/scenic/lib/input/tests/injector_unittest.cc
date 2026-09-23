// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.ui.pointerinjector/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointerinjector/cpp/wire.h>
#include <lib/async/default.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/syslog/cpp/macros.h>

#include <tuple>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/scenic/lib/input/touch_injector.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/math.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

using Phase = fuchsia_ui_pointerinjector::EventPhase;
using InjectionEvent = fuchsia_ui_pointerinjector::Event;
using StreamId = scenic_impl::input::StreamId;

namespace input::test {

namespace {

class DeviceEventHandler : public fidl::AsyncEventHandler<fuchsia_ui_pointerinjector::Device> {
 public:
  DeviceEventHandler() = default;
  explicit DeviceEventHandler(fit::function<void(fidl::UnbindInfo)> on_error)
      : on_error_(std::move(on_error)) {}

  void on_fidl_error(fidl::UnbindInfo error) override {
    error_callback_fired = true;
    status = error.status();
    if (on_error_) {
      on_error_(error);
    }
  }

  bool error_callback_fired = false;
  zx_status_t status = ZX_OK;

 private:
  fit::function<void(fidl::UnbindInfo)> on_error_;
};

// clang-format off
static constexpr std::array<float, 9> kIdentityMatrix = {
  1, 0, 0, // first column
  0, 1, 0, // second column
  0, 0, 1, // third column
};
// clang-format on

scenic_impl::input::InjectorSettings InjectorSettingsTemplate() {
  return {.dispatch_policy = fuchsia_ui_pointerinjector::wire::DispatchPolicy::kExclusiveTarget,
          .device_id = 1,
          .device_type = fuchsia_ui_pointerinjector::wire::DeviceType::kTouch,
          .context_koid = 1,
          .target_koid = 2};
}

scenic_impl::input::Viewport ViewportTemplate() {
  return {
      .extents = std::array<std::array<float, 2>, 2>{{{0, 0}, {1000, 1000}}},
      .context_from_viewport_transform = utils::ColumnMajorMat3ArrayToMat4(kIdentityMatrix),
  };
}

std::shared_ptr<view_tree::SnapshotHolder> ValidSnapshotHolder() {
  auto holder = std::make_shared<view_tree::SnapshotHolder>();
  auto snapshot = std::make_shared<view_tree::Snapshot>();
  snapshot->sequence_number = 1;
  snapshot->root = 1;
  snapshot->view_tree[1] = {.children = {2}};
  snapshot->view_tree[2] = {.parent = 1};
  holder->SetSnapshot(snapshot);
  return holder;
}

scenic_impl::input::TouchInjector CreateTouchInjector(
    std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder, inspect::Node inspect_node,
    scenic_impl::input::InjectorSettings settings, scenic_impl::input::Viewport viewport,
    fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
    fit::function<void(scenic_impl::input::InternalTouchEvent, StreamId stream_id,
                       const view_tree::Snapshot& snapshot)>
        inject,
    fit::function<void()> on_channel_closed = [] {}) {
  return scenic_impl::input::TouchInjector(
      std::move(snapshot_holder), std::move(inspect_node), std::move(settings), std::move(viewport),
      std::move(device), std::move(inject), std::move(on_channel_closed));
}

InjectionEvent InjectionEventTemplate() {
  fuchsia_ui_pointerinjector::PointerSample pointer_sample;
  pointer_sample.pointer_id(2222);
  pointer_sample.phase(Phase::kChange);
  pointer_sample.position_in_viewport(std::array<float, 2>{333.f, 444.f});

  InjectionEvent event;
  event.timestamp(1111);
  event.data(fuchsia_ui_pointerinjector::Data::WithPointerSample(std::move(pointer_sample)));
  return event;
}

}  // namespace

class InjectorTestP : public gtest::TestLoopFixture, public testing::WithParamInterface<bool> {
 public:
  bool use_inject_events() const { return GetParam(); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

INSTANTIATE_TEST_SUITE_P(InjectorTest, InjectorTestP, testing::Bool());

TEST_P(InjectorTestP, InjectedEvents_ShouldTriggerTheInjectLambda) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  uint32_t num_injections = 0;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[&num_injections](auto...) { ++num_injections; },
      /*on_channel_closed=*/[] {});

  {  // Inject one event.
    bool injection_callback_fired = false;
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->phase(Phase::kAdd);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();
    if (!use_inject_events()) {
      EXPECT_TRUE(injection_callback_fired);
    }
  }

  EXPECT_EQ(num_injections, 1u);

  {  // Inject CHANGE event.
    bool injection_callback_fired = false;
    std::vector<InjectionEvent> events;
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->phase(Phase::kChange);
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();
    if (!use_inject_events()) {
      EXPECT_TRUE(injection_callback_fired);
    }

    EXPECT_EQ(num_injections, 2u);
  }

  {  // Inject remove event.
    bool injection_callback_fired = false;
    std::vector<InjectionEvent> events;
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->phase(Phase::kRemove);
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();
    if (!use_inject_events()) {
      EXPECT_TRUE(injection_callback_fired);
    }
  }

  EXPECT_EQ(num_injections, 3u);
  EXPECT_FALSE(event_handler.error_callback_fired);
}

TEST_P(InjectorTestP, InjectionWithNoEvent_ShouldCloseChannel) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [](auto...) {},
      /*on_channel_closed=*/[] {});

  bool injection_callback_fired = false;
  // Inject nothing.
  Inject(injector, {}, [&injection_callback_fired] { injection_callback_fired = true; });
  RunLoopUntilIdle();

  EXPECT_FALSE(injection_callback_fired);
  EXPECT_TRUE(event_handler.error_callback_fired);
}

TEST_P(InjectorTestP, ClientClosingChannel_ShouldTriggerCancelEvents_ForEachOngoingStream) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  std::vector<uint32_t> cancelled_streams;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [&cancelled_streams](const scenic_impl::input::InternalTouchEvent& event, StreamId,
                           const view_tree::Snapshot&) {
        if (event.phase == scenic_impl::input::Phase::kCancel)
          cancelled_streams.push_back(event.pointer_id);
      },
      /*on_channel_closed=*/[] {});

  // Start three streams and end one.
  {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(1);
    event.data()->pointer_sample()->phase(Phase::kAdd);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
  }
  {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(2);
    event.data()->pointer_sample()->phase(Phase::kAdd);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
  }
  {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(3);
    event.data()->pointer_sample()->phase(Phase::kAdd);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
  }
  {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(1);
    event.data()->pointer_sample()->phase(Phase::kRemove);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
  }

  // Close the client side channel.
  injector = {};
  RunLoopUntilIdle();

  // Should receive two CANCEL events, since there should be two ongoing streams.
  EXPECT_FALSE(event_handler.error_callback_fired);
  EXPECT_THAT(cancelled_streams, testing::UnorderedElementsAre(2, 3));
}

TEST_P(InjectorTestP, ServerClosingChannel_ShouldTriggerCancelEvents_ForEachOngoingStream) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  std::vector<uint32_t> cancelled_streams;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [&cancelled_streams](const scenic_impl::input::InternalTouchEvent& event, StreamId,
                           const view_tree::Snapshot&) {
        if (event.phase == scenic_impl::input::Phase::kCancel)
          cancelled_streams.push_back(event.pointer_id);
      },
      /*on_channel_closed=*/[] {});

  // Start three streams and end one.
  {
    std::vector<InjectionEvent> events;
    {
      InjectionEvent event = InjectionEventTemplate();
      event.data()->pointer_sample()->pointer_id(1);
      event.data()->pointer_sample()->phase(Phase::kAdd);
      events.emplace_back(std::move(event));
    }
    {
      InjectionEvent event = InjectionEventTemplate();
      event.data()->pointer_sample()->pointer_id(2);
      event.data()->pointer_sample()->phase(Phase::kAdd);
      events.emplace_back(std::move(event));
    }
    {
      InjectionEvent event = InjectionEventTemplate();
      event.data()->pointer_sample()->pointer_id(3);
      event.data()->pointer_sample()->phase(Phase::kAdd);
      events.emplace_back(std::move(event));
    }
    {
      InjectionEvent event = InjectionEventTemplate();
      event.data()->pointer_sample()->pointer_id(1);
      event.data()->pointer_sample()->phase(Phase::kRemove);
      events.emplace_back(std::move(event));
    }
    Inject(injector, {std::move(events)}, [] {});
  }

  // Inject an event with missing fields to cause the channel to close.
  {
    std::vector<InjectionEvent> events;
    events.emplace_back();
    Inject(injector, std::move(events), [] {});
  }
  RunLoopUntilIdle();

  EXPECT_TRUE(event_handler.error_callback_fired);
  // Should receive CANCEL events for the two ongoing streams; 2 and 3.
  EXPECT_THAT(cancelled_streams, testing::UnorderedElementsAre(2, 3));
}

TEST_P(InjectorTestP, InjectionOfEmptyEvent_ShouldCloseChannel) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  bool injection_lambda_fired = false;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [&injection_lambda_fired](auto...) { injection_lambda_fired = true; },
      /*on_channel_closed=*/[] {});

  bool injection_callback_fired = false;
  InjectionEvent event;
  std::vector<InjectionEvent> events;
  events.emplace_back(std::move(event));
  Inject(injector, {std::move(events)},
         [&injection_callback_fired] { injection_callback_fired = true; });
  RunLoopUntilIdle();

  EXPECT_FALSE(injection_lambda_fired);
  EXPECT_FALSE(injection_callback_fired);
  EXPECT_TRUE(event_handler.error_callback_fired);
}

TEST_P(InjectorTestP, ClientClosingChannel_ShouldTriggerOnChannelClosedLambda) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  bool on_channel_closed_callback_fired = false;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[](auto...) {},
      /*on_channel_closed=*/
      [&on_channel_closed_callback_fired] { on_channel_closed_callback_fired = true; });

  // Close the client side channel.
  injector = {};
  RunLoopUntilIdle();

  EXPECT_FALSE(event_handler.error_callback_fired);
  EXPECT_TRUE(on_channel_closed_callback_fired);
}

TEST_P(InjectorTestP, ServerClosingChannel_ShouldTriggerOnChannelClosedLambda) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  bool on_channel_closed_callback_fired = false;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[](auto...) {},
      /*on_channel_closed=*/
      [&on_channel_closed_callback_fired] { on_channel_closed_callback_fired = true; });

  // Inject an event with missing fields to cause the channel to close.
  {
    std::vector<InjectionEvent> events;
    events.emplace_back();
    Inject(injector, std::move(events), [] {});
  }
  RunLoopUntilIdle();

  EXPECT_TRUE(event_handler.error_callback_fired);
  EXPECT_EQ(event_handler.status, ZX_ERR_INVALID_ARGS);
  EXPECT_TRUE(on_channel_closed_callback_fired);
}

TEST_P(InjectorTestP, InjectionWithBadConnectivity_ShouldCloseChannel) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto holder = ValidSnapshotHolder();
  uint32_t num_cancel_events = 0;
  auto injector_impl = CreateTouchInjector(
      holder, inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [&num_cancel_events](const scenic_impl::input::InternalTouchEvent& event, StreamId,
                           const view_tree::Snapshot&) {
        num_cancel_events += event.phase == scenic_impl::input::Phase::kCancel ? 1 : 0;
      },
      /*on_channel_closed=*/[] {});

  // Start event stream while connectivity is good.
  {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->phase(Phase::kAdd);
    event.data()->pointer_sample()->pointer_id(1);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  // Connectivity was good. No problems.
  EXPECT_FALSE(event_handler.error_callback_fired);

  // Inject with bad connectivity.
  {
    auto bad_snapshot = std::make_shared<view_tree::Snapshot>();
    bad_snapshot->sequence_number = 100;
    holder->SetSnapshot(bad_snapshot);
    bool injection_callback_fired = false;
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->phase(Phase::kChange);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();
    EXPECT_FALSE(injection_callback_fired);
  }

  // Connectivity was bad, so channel should be closed and an extra CANCEL event should have been
  // injected for each ongoing stream.
  EXPECT_EQ(num_cancel_events, 1u);
  EXPECT_TRUE(event_handler.error_callback_fired);
  EXPECT_EQ(event_handler.status, ZX_ERR_BAD_STATE);
}

// Class for testing parameterized injection of invalid events.
// Takes an int that determines which field gets deleted and a bool for Inject/InjectEvents.
class InjectorInvalidEventsTest : public gtest::TestLoopFixture,
                                  public testing::WithParamInterface<std::tuple<int, bool>> {
 public:
  int GetMissingField() const { return std::get<0>(GetParam()); }
  bool use_inject_events() const { return std::get<1>(GetParam()); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

INSTANTIATE_TEST_SUITE_P(InjectEventWithMissingField_ShouldCloseChannel, InjectorInvalidEventsTest,
                         testing::Combine(testing::Range(0, 3), testing::Bool()));

TEST_P(InjectorInvalidEventsTest, InjectEventWithMissingField_ShouldCloseChannel) {
  fuchsia_ui_pointerinjector::PointerSample pointer_sample;
  pointer_sample.pointer_id(2222);
  pointer_sample.phase(Phase::kChange);
  pointer_sample.position_in_viewport(std::array<float, 2>{333.f, 444.f});

  switch (GetMissingField()) {
    case 0:
      pointer_sample.pointer_id().reset();
      break;
    case 1:
      pointer_sample.phase().reset();
      break;
    case 2:
      pointer_sample.position_in_viewport().reset();
      break;
  }

  InjectionEvent event;
  event.timestamp(1111);
  event.data(fuchsia_ui_pointerinjector::Data::WithPointerSample(std::move(pointer_sample)));

  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [](auto...) {},
      /*on_channel_closed=*/[] {});

  bool injection_callback_fired = false;
  std::vector<InjectionEvent> events;
  events.emplace_back(std::move(event));
  Inject(injector, {std::move(events)},
         [&injection_callback_fired] { injection_callback_fired = true; });
  RunLoopUntilIdle();

  EXPECT_FALSE(injection_callback_fired);
  EXPECT_TRUE(event_handler.error_callback_fired);
  EXPECT_EQ(event_handler.status, ZX_ERR_INVALID_ARGS);
}

// Class for testing different event streams.
// Each invocation gets a vector of pairs of pointer ids and Phases, representing pointer streams,
// and a bool for Inject/InjectEvents.
class InjectorGoodEventStreamTest
    : public gtest::TestLoopFixture,
      public testing::WithParamInterface<
          std::tuple<std::vector<std::pair</*pointer_id*/ uint32_t, Phase>>, bool>> {
 public:
  std::vector<std::pair<uint32_t, Phase>> GetEventStream() const { return std::get<0>(GetParam()); }
  bool use_inject_events() const { return std::get<1>(GetParam()); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

static std::vector<std::vector<std::pair<uint32_t, Phase>>> GoodStreamTestData() {
  // clang-format off
  return {
    {{1, Phase::kAdd}, {1, Phase::kRemove}},                         // 0: one pointer trivial
    {{1, Phase::kAdd}, {1, Phase::kChange}, {1, Phase::kRemove}},     // 1: one pointer minimal all phases
    {{1, Phase::kAdd}, {1, Phase::kCancel}},                         // 2: one pointer trivial cancelled
    {{1, Phase::kAdd}, {1, Phase::kChange}, {1, Phase::kCancel}},     // 3: one pointer minimal all phases cancelled
    {{1, Phase::kAdd}, {1, Phase::kChange}, {1, Phase::kCancel},
     {2, Phase::kAdd}, {2, Phase::kChange}, {2, Phase::kCancel}},     // 4: two pointers successive streams
    {{2, Phase::kAdd},    {1, Phase::kAdd},    {2, Phase::kChange},
     {1, Phase::kChange}, {1, Phase::kCancel}, {2, Phase::kCancel}},  // 5: two pointer interleaved
  };
  // clang-format on
}

INSTANTIATE_TEST_SUITE_P(InjectionWithGoodEventStream_ShouldHaveNoProblems_CombinedEvents,
                         InjectorGoodEventStreamTest,
                         testing::Combine(testing::ValuesIn(GoodStreamTestData()),
                                          testing::Bool()));

INSTANTIATE_TEST_SUITE_P(InjectionWithGoodEventStream_ShouldHaveNoProblems_SeparateEvents,
                         InjectorGoodEventStreamTest,
                         testing::Combine(testing::ValuesIn(GoodStreamTestData()),
                                          testing::Bool()));

// Inject a valid event stream in a single Inject() call.
TEST_P(InjectorGoodEventStreamTest,
       InjectionWithGoodEventStream_ShouldHaveNoProblems_CombinedEvents) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[](auto...) {},
      /*on_channel_closed=*/[] {});

  bool injection_callback_fired = false;
  std::vector<InjectionEvent> events;
  for (auto [pointer_id, phase] : GetEventStream()) {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(pointer_id);
    event.data()->pointer_sample()->phase(phase);
    events.emplace_back(std::move(event));
  }
  Inject(injector, {std::move(events)},
         [&injection_callback_fired] { injection_callback_fired = true; });
  RunLoopUntilIdle();

  if (!use_inject_events()) {
    EXPECT_TRUE(injection_callback_fired);
  }
  EXPECT_FALSE(event_handler.error_callback_fired);
}

// Inject a valid event stream in multiple Inject() calls.
TEST_P(InjectorGoodEventStreamTest,
       InjectionWithGoodEventStream_ShouldHaveNoProblems_SeparateEvents) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/
      [](auto...) {},
      /*on_channel_closed=*/[] {});

  for (auto [pointer_id, phase] : GetEventStream()) {
    bool injection_callback_fired = false;
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(pointer_id);
    event.data()->pointer_sample()->phase(phase);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();

    if (!use_inject_events()) {
      EXPECT_TRUE(injection_callback_fired);
    }
    ASSERT_FALSE(event_handler.error_callback_fired);
  }
}

// Bad event streams.
// Each invocation gets a vector of pairs of pointer ids and Phases, representing pointer streams,
// and a bool for Inject/InjectEvents.
class InjectorBadEventStreamTest
    : public gtest::TestLoopFixture,
      public testing::WithParamInterface<
          std::tuple<std::vector<std::pair</*pointer_id*/ uint32_t, Phase>>, bool>> {
 public:
  std::vector<std::pair<uint32_t, Phase>> GetEventStream() const { return std::get<0>(GetParam()); }
  bool use_inject_events() const { return std::get<1>(GetParam()); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

static std::vector<std::vector<std::pair<uint32_t, Phase>>> BadStreamTestData() {
  // clang-format off
  return {
    {{1, Phase::kChange}},                                       // 0: one pointer non-add initial event
    {{1, Phase::kRemove}},                                       // 1: one pointer non-add initial event
    {{1, Phase::kAdd}, {1, Phase::kAdd}},                         // 2: one pointer double add
    {{1, Phase::kAdd}, {1, Phase::kChange}, {1, Phase::kAdd}},     // 3: one pointer double add mid-stream
    {{1, Phase::kAdd}, {1, Phase::kRemove}, {1, Phase::kRemove}},  // 4: one pointer double remove
    {{1, Phase::kAdd}, {1, Phase::kRemove}, {1, Phase::kChange}},  // 5: one pointer event after remove
    {{1, Phase::kAdd}, {1, Phase::kChange},
     {1, Phase::kRemove}, {2, Phase::kAdd}, {2, Phase::kAdd}},     // 6: two pointer faulty stream after correct stream
    {{1, Phase::kAdd}, {2, Phase::kAdd},
     {2, Phase::kChange}, {2, Phase::kRemove}, {1, Phase::kAdd}},  // 7  two pointer faulty stream interleaved with correct stream
  };
  // clang-format on
}

INSTANTIATE_TEST_SUITE_P(InjectionWithBadEventStream_ShouldCloseChannel_CombinedEvents,
                         InjectorBadEventStreamTest,
                         testing::Combine(testing::ValuesIn(BadStreamTestData()), testing::Bool()));

INSTANTIATE_TEST_SUITE_P(InjectionWithBadEventStream_ShouldCloseChannel_SeparateEvents,
                         InjectorBadEventStreamTest,
                         testing::Combine(testing::ValuesIn(BadStreamTestData()), testing::Bool()));

// Inject an invalid event stream in a single Inject() call.
TEST_P(InjectorBadEventStreamTest, InjectionWithBadEventStream_ShouldCloseChannel_CombinedEvents) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[](auto...) {},
      /*on_channel_closed=*/[] {});

  // Run event stream.
  std::vector<InjectionEvent> events;
  for (auto [pointer_id, phase] : GetEventStream()) {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(pointer_id);
    event.data()->pointer_sample()->phase(phase);
    events.emplace_back(std::move(event));
  }
  Inject(injector, {std::move(events)}, [] {});
  RunLoopUntilIdle();

  EXPECT_TRUE(event_handler.error_callback_fired);
  EXPECT_EQ(event_handler.status, ZX_ERR_BAD_STATE);
}

// Inject an invalid event stream in multiple Inject() calls.
TEST_P(InjectorBadEventStreamTest, InjectionWithBadEventStream_ShouldCloseChannel_SeparateEvents) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[](auto...) {},
      /*on_channel_closed=*/[] {});

  // Run event stream.
  for (auto [pointer_id, phase] : GetEventStream()) {
    InjectionEvent event = InjectionEventTemplate();
    event.data()->pointer_sample()->pointer_id(pointer_id);
    event.data()->pointer_sample()->phase(phase);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_TRUE(event_handler.error_callback_fired);
  EXPECT_EQ(event_handler.status, ZX_ERR_BAD_STATE);
}

TEST_P(InjectorTestP, InjectedViewport_ShouldNotTriggerInjectLambda) {
  // Set up an isolated Injector.
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  bool inject_lambda_fired = false;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[&inject_lambda_fired](auto...) { inject_lambda_fired = true; },
      /*on_channel_closed=*/[] {});

  {
    bool injection_callback_fired = false;
    fuchsia_ui_pointerinjector::Viewport viewport;
    viewport.extents(std::array<std::array<float, 2>, 2>{{{-242.f, -383.f}, {124.f, 252.f}}});
    viewport.viewport_to_context_transform(kIdentityMatrix);
    InjectionEvent event;
    event.timestamp(1);
    event.data(fuchsia_ui_pointerinjector::Data::WithViewport(std::move(viewport)));

    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector, {std::move(events)},
           [&injection_callback_fired] { injection_callback_fired = true; });
    RunLoopUntilIdle();
    if (!use_inject_events()) {
      EXPECT_TRUE(injection_callback_fired);
    }
  }

  RunLoopUntilIdle();

  EXPECT_FALSE(inject_lambda_fired);
  EXPECT_FALSE(event_handler.error_callback_fired);
}

// Parameterized tests for malformed viewport arguments.
// Use pairs of optional extents and matrices. Because test parameters must be copyable.
using ViewportPair = std::pair<std::optional<std::array<std::array<float, 2>, 2>>,
                               std::optional<std::array<float, 9>>>;
class InjectorBadViewportTest : public gtest::TestLoopFixture,
                                public testing::WithParamInterface<std::tuple<ViewportPair, bool>> {
 public:
  ViewportPair GetViewportPair() const { return std::get<0>(GetParam()); }
  bool use_inject_events() const { return std::get<1>(GetParam()); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

static std::vector<ViewportPair> BadViewportTestData() {
  std::vector<ViewportPair> bad_viewports;
  {  // 0: No extents.
    ViewportPair pair;
    pair.second.emplace(kIdentityMatrix);
    bad_viewports.emplace_back(pair);
  }
  {  // 1: No viewport_to_context_transform.
    ViewportPair pair;
    pair.first = {{/*min*/ {0, 0}, /*max*/ {10, 10}}};
    bad_viewports.emplace_back(pair);
  }
  {  // 2: Malformed extents: Min bigger than max.
    ViewportPair pair;
    pair.first = {{/*min*/ {-100, 100}, /*max*/ {100, -100}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 3: Malformed extents: Min equal to max.
    ViewportPair pair;
    pair.first = {{/*min*/ {0, -100}, /*max*/ {0, 100}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 4: Malformed extents: Contains NaN
    ViewportPair pair;
    pair.first = {{/*min*/ {0, 0}, /*max*/ {100, std::numeric_limits<double>::quiet_NaN()}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 5: Malformed extents: Contains Inf
    ViewportPair pair;
    pair.first = {{/*min*/ {0, 0}, /*max*/ {100, std::numeric_limits<double>::infinity()}}};
    pair.second = kIdentityMatrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 6: Malformed transform: Non-invertible matrix
    // clang-format off
    const std::array<float, 9> non_invertible_matrix = {
      1, 0, 0,
      1, 0, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ {0, 0}, /*max*/ {10, 10}}}};
    pair.second = non_invertible_matrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 7: Malformed transform: Contains NaN
    // clang-format off
    const std::array<float, 9> nan_matrix = {
      1, std::numeric_limits<double>::quiet_NaN(), 0,
      0, 1, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ {0, 0}, /*max*/ {10, 10}}}};
    pair.second = nan_matrix;
    bad_viewports.emplace_back(pair);
  }
  {  // 8: Malformed transform: Contains Inf
    // clang-format off
    const std::array<float, 9> inf_matrix = {
      1, std::numeric_limits<double>::infinity(), 0,
      0, 1, 0,
      0, 0, 1,
    };
    // clang-format on
    ViewportPair pair;
    pair.first = {{{/*min*/ {0, 0}, /*max*/ {10, 10}}}};
    pair.second = inf_matrix;
    bad_viewports.emplace_back(pair);
  }
  return bad_viewports;
}

INSTANTIATE_TEST_SUITE_P(InjectBadViewport_ShouldCloseChannel, InjectorBadViewportTest,
                         testing::Combine(testing::ValuesIn(BadViewportTestData()),
                                          testing::Bool()));

TEST_P(InjectorBadViewportTest, InjectBadViewport_ShouldCloseChannel) {
  DeviceEventHandler event_handler;
  auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector(std::move(endpoints.client),
                                                            dispatcher(), &event_handler);

  bool inject_lambda_fired = false;
  auto injector_impl = CreateTouchInjector(
      ValidSnapshotHolder(), inspect::Node(), InjectorSettingsTemplate(), ViewportTemplate(),
      std::move(endpoints.server),
      /*inject=*/[&inject_lambda_fired](auto...) { inject_lambda_fired = true; },
      /*on_channel_closed=*/[] {});

  InjectionEvent event;
  event.timestamp(1);
  ViewportPair params = GetViewportPair();
  fuchsia_ui_pointerinjector::Viewport viewport;
  if (params.first)
    viewport.extents(params.first.value());
  if (params.second)
    viewport.viewport_to_context_transform(params.second.value());
  event.data(fuchsia_ui_pointerinjector::Data::WithViewport(std::move(viewport)));

  std::vector<InjectionEvent> events;
  events.emplace_back(std::move(event));
  bool injection_callback_fired = false;
  Inject(injector, {std::move(events)},
         [&injection_callback_fired] { injection_callback_fired = true; });

  RunLoopUntilIdle();
  EXPECT_FALSE(injection_callback_fired);
  EXPECT_TRUE(event_handler.error_callback_fired);
}

class InjectorInspectionTest : public gtest::TestLoopFixture,
                               public testing::WithParamInterface<bool> {
 public:
  bool use_inject_events() const { return GetParam(); }

  void Inject(fidl::Client<fuchsia_ui_pointerinjector::Device>& injector,
              std::vector<InjectionEvent> events, std::function<void()> callback) {
    if (use_inject_events()) {
      auto result = injector->InjectEvents({{.events = std::move(events)}});
      EXPECT_TRUE(result.is_ok());
    } else {
      injector->Inject({{.events = std::move(events)}})
          .Then([callback = std::move(callback)](
                    fidl::Result<fuchsia_ui_pointerinjector::Device::Inject>& result) {
            if (result.is_ok()) {
              callback();
            }
          });
    }
  }

 protected:
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_ui_pointerinjector::Device>::Create();
    injector_.Bind(std::move(endpoints.client), dispatcher(), &event_handler_);
    injector_impl_.emplace(
        ValidSnapshotHolder(), inspector_.GetRoot().CreateChild("injector"),
        InjectorSettingsTemplate(), ViewportTemplate(), std::move(endpoints.server),
        /*inject=*/[this](auto...) { ++num_injections_; },
        /*on_channel_closed=*/[] {});
  }

  std::pair<inspect::Hierarchy, const inspect::Hierarchy*> ReadHierarchyFromInspector() {
    fpromise::result<inspect::Hierarchy> result;
    fpromise::single_threaded_executor exec;
    exec.schedule_task(
        inspect::ReadFromInspector(inspector_).then([&](fpromise::result<inspect::Hierarchy>& res) {
          result = std::move(res);
        }));
    exec.run();

    inspect::Hierarchy root = result.take_value();
    const inspect::Hierarchy* hierarchy = root.GetByPath({"injector"});
    FX_DCHECK(hierarchy);
    return {std::move(root), hierarchy};
  }

  uint64_t GetInjectionsAtMinute(uint64_t minute) {
    const auto [root, parent] = ReadHierarchyFromInspector();
    const inspect::Hierarchy* node = parent->GetByPath({kHistoryNodeName});
    FX_CHECK(node);
    const inspect::UintPropertyValue* count = node->node().get_property<inspect::UintPropertyValue>(
        "Events at minute " + std::to_string(minute));
    if (count) {
      return count->value();
    } else {
      FX_LOGS(INFO) << "Found no data for minute " << minute;
      return 0;
    }
  }

  uint64_t GetTotalInjections() {
    const auto [root, parent] = ReadHierarchyFromInspector();
    const inspect::Hierarchy* node = parent->GetByPath({kHistoryNodeName});
    FX_CHECK(node);
    const inspect::UintPropertyValue* total =
        node->node().get_property<inspect::UintPropertyValue>("Total");
    FX_CHECK(total);
    return total->value();
  }

  const std::string kHistoryNodeName =
      "Last " + std::to_string(scenic_impl::input::InjectorInspector::kNumMinutesOfHistory) +
      " minutes of injected events";
  inspect::Inspector inspector_;
  DeviceEventHandler event_handler_;
  fidl::Client<fuchsia_ui_pointerinjector::Device> injector_;
  uint64_t num_injections_ = 0;
  std::optional<scenic_impl::input::TouchInjector> injector_impl_;
  utils::ScopedThreadDispatcherSetter dispatcher_setter_{dispatcher(), dispatcher()};
};

INSTANTIATE_TEST_SUITE_P(InjectorInspectionTest, InjectorInspectionTest, testing::Bool());

TEST_P(InjectorInspectionTest, InspectHistory) {
  const uint64_t kMaxNum = scenic_impl::input::InjectorInspector::kNumMinutesOfHistory;
  ASSERT_TRUE(kMaxNum > 2) << "This test assumes a minimum length of history";

  const uint64_t start_minute = Now().get() / zx::min(1).get();

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 0u);
  EXPECT_EQ(GetTotalInjections(), 0u);

  // Inject events. Each one should register in inspect.
  {
    InjectionEvent event = InjectionEventTemplate();
    event.timestamp(Now().get());
    event.data()->pointer_sample()->phase(Phase::kAdd);
    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 1u);
  EXPECT_EQ(GetTotalInjections(), 1u);

  {
    std::vector<InjectionEvent> events;
    InjectionEvent event = InjectionEventTemplate();
    event.timestamp(Now().get());
    event.data()->pointer_sample()->phase(Phase::kChange);
    events.emplace_back(std::move(event));
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 2u);
  EXPECT_EQ(GetTotalInjections(), 2u);

  {
    std::vector<InjectionEvent> events;
    InjectionEvent event = InjectionEventTemplate();
    event.timestamp(Now().get());
    event.data()->pointer_sample()->phase(Phase::kChange);
    events.emplace_back(std::move(event));
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 3u);
  EXPECT_EQ(GetTotalInjections(), 3u);

  {  // Inject VIEWPORT event. It should not be reflected in the injection stats.
    fuchsia_ui_pointerinjector::Viewport viewport;
    viewport.extents(std::array<std::array<float, 2>, 2>{{{-242.f, -383.f}, {124.f, 252.f}}});
    viewport.viewport_to_context_transform(kIdentityMatrix);
    InjectionEvent event;
    event.timestamp(1);
    event.data(fuchsia_ui_pointerinjector::Data::WithViewport(std::move(viewport)));

    std::vector<InjectionEvent> events;
    events.emplace_back(std::move(event));
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 3u);
  EXPECT_EQ(GetTotalInjections(), 3u);

  // Roll forward one minute, inject an event and observe that history has updated correctly.
  RunLoopFor(zx::min(1));
  {
    std::vector<InjectionEvent> events;
    InjectionEvent event = InjectionEventTemplate();
    event.timestamp(Now().get());
    event.data()->pointer_sample()->phase(Phase::kChange);
    events.emplace_back(std::move(event));
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 3u);
  EXPECT_EQ(GetInjectionsAtMinute(start_minute + 1), 1u);
  EXPECT_EQ(GetTotalInjections(), 4u);

  // Roll forward one less than the size of the ringbuffer. Now the start minute should have
  // disappeared, but not the second minute.
  RunLoopFor(zx::min(kMaxNum - 1));

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 0u);
  EXPECT_EQ(GetInjectionsAtMinute(start_minute + 1), 1u);
  EXPECT_EQ(GetInjectionsAtMinute(start_minute + kMaxNum), 0u);
  EXPECT_EQ(GetTotalInjections(), 1u);

  {
    std::vector<InjectionEvent> events;
    {
      InjectionEvent event = InjectionEventTemplate();
      event.timestamp(Now().get());
      event.data()->pointer_sample()->phase(Phase::kChange);
      events.emplace_back(std::move(event));
    }
    {
      InjectionEvent event = InjectionEventTemplate();
      event.timestamp(Now().get());
      event.data()->pointer_sample()->phase(Phase::kChange);
      events.emplace_back(std::move(event));
    }
    Inject(injector_, {std::move(events)}, [] {});
    RunLoopUntilIdle();
  }

  EXPECT_EQ(GetInjectionsAtMinute(start_minute), 0u);
  EXPECT_EQ(GetInjectionsAtMinute(start_minute + 1), 1u);
  EXPECT_EQ(GetInjectionsAtMinute(start_minute + kMaxNum), 2u);
  EXPECT_EQ(GetTotalInjections(), 3u);
}

}  // namespace input::test
