// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointerinjector/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <lib/ui/scenic/cpp/view_ref_pair.h>
#include <zircon/status.h>

#include <algorithm>
#include <memory>
#include <utility>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/ui/scenic/tests/utils/blocking_present.h"
#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"
#include "src/ui/scenic/tests/utils/scenic_ctf_test_base.h"
#include "src/ui/scenic/tests/utils/simple_watcher_client.h"
#include "src/ui/scenic/tests/utils/utils.h"

// These tests exercise the integration between Flatland and the InputSystem, including the
// View-to-View transform logic between the injection point and the receiver.
// Setup:
// - The test fixture sets up the display + the root session and view.
// - Injection done in context View Space, with fuchsia.ui.pointerinjector
// - Target(s) specified by View (using view ref koids)
// - Dispatch done to fuchsia.ui.pointer.TouchSource in receiver View Space.

namespace integration_tests {

namespace fuc = fuchsia_ui_composition;
namespace fup = fuchsia_ui_pointer;
namespace fupi = fuchsia_ui_pointerinjector;
namespace fuv = fuchsia_ui_views;

using TouchInteractionStatus = fup::TouchInteractionStatus;
using DispatchPolicy = fupi::DispatchPolicy;
using Viewport = fupi::Viewport;
using ChildViewWatcher = fuc::ChildViewWatcher;
using ContentId = fuc::ContentId;
using Flatland = fuc::Flatland;
using FlatlandDisplay = fuc::FlatlandDisplay;
using Orientation = fuc::Orientation;
using ParentViewportWatcher = fuc::ParentViewportWatcher;
using TransformId = fuc::TransformId;
using ViewBoundProtocols = fuc::ViewBoundProtocols;
using ViewportProperties = fuc::ViewportProperties;
using HitRegion = fuc::HitRegion;
using HitTestInteraction = fuc::HitTestInteraction;
using EventPhase = fup::EventPhase;
using TouchEvent = fup::TouchEvent;
using TouchResponse = fup::TouchResponse;
using TouchResponseType = fup::TouchResponseType;
using TouchSourceV2 = fup::TouchSourceV2;
using ViewportCreationToken = fuv::ViewportCreationToken;
using ViewRef = fuv::ViewRef;
using Rect = fuchsia_math::Rect;
using RectF = fuchsia_math::RectF;
using SizeU = fuchsia_math::SizeU;
using Vec = fuchsia_math::Vec;
using VecF = fuchsia_math::VecF;

namespace {

const fuc::TransformId kRootTransform(1);
const fuc::TransformId kViewportTransform(2);
const fuc::ContentId kRootContentId(1);

std::array<float, 2> TransformPointerCoords(std::array<float, 2> pointer, const Mat3& transform) {
  const Vec3 homogenous_pointer = {pointer[0], pointer[1], 1};
  Vec3 transformed_pointer = transform * homogenous_pointer;
  FX_CHECK(transformed_pointer[2] != 0);
  const Vec3& homogenized = transformed_pointer / transformed_pointer[2];
  return {homogenized[0], homogenized[1]};
}

void ExpectEqualPointer(const std::optional<fup::TouchPointerSample>& pointer_sample,
                        const std::optional<std::array<float, 9>>& viewport_to_view_transform,
                        fup::EventPhase expected_phase, float expected_x, float expected_y,
                        uint32_t line_number) {
  ASSERT_TRUE(pointer_sample.has_value(), "Line: %d", line_number);
  EXPECT_EQ(pointer_sample->phase(), expected_phase, "Line: %d", line_number);
  ASSERT_TRUE(viewport_to_view_transform.has_value(), "Line: %d", line_number);
  const Mat3 transform_matrix = ArrayToMat3(viewport_to_view_transform.value());
  ASSERT_TRUE(pointer_sample->position_in_viewport().has_value(), "Line: %d", line_number);
  const std::array<float, 2> transformed_pointer =
      TransformPointerCoords(pointer_sample->position_in_viewport().value(), transform_matrix);
  EXPECT_TRUE(CmpFloatingValues(transformed_pointer[0], expected_x), "Line: %d", line_number);
  EXPECT_TRUE(CmpFloatingValues(transformed_pointer[1], expected_y), "Line: %d", line_number);
}

}  // namespace

#define EXPECT_EQ_POINTER(pointer_sample, viewport_to_view_transform, expected_phase, expected_x, \
                          expected_y)                                                             \
  ExpectEqualPointer(pointer_sample, viewport_to_view_transform, expected_phase, expected_x,      \
                     expected_y, __LINE__)

class TouchSourceV2Client : public fidl::AsyncEventHandler<fup::TouchSourceV2> {
 public:
  TouchSourceV2Client(fidl::ClientEnd<fup::TouchSourceV2> client_end,
                      async_dispatcher_t* dispatcher, std::vector<fup::TouchEvent>& out_events)
      : client_(std::move(client_end), dispatcher, this), out_events_(out_events) {}

  void on_fidl_error(fidl::UnbindInfo info) override { is_bound_ = false; }
  void handle_unknown_event(fidl::UnknownEventMetadata<fup::TouchSourceV2> metadata) override {}

  void OnTouchEvents(fidl::Event<fup::TouchSourceV2::OnTouchEvents>& event) override {
    std::move(event.events().begin(), event.events().end(), std::back_inserter(out_events_));
    EXPECT_TRUE(
        client_->AcknowledgeEvents({{.last_acknowledged_event_stamp = event.last_event_stamp()}})
            .is_ok());
  }

  bool is_bound() const { return is_bound_ && client_.is_valid(); }
  fidl::Client<fup::TouchSourceV2>& operator->() { return client_; }

 private:
  bool is_bound_ = true;
  fidl::Client<fup::TouchSourceV2> client_;
  std::vector<fup::TouchEvent>& out_events_;
};

class FlatlandTouchIntegrationTest : public ScenicCtfTest {
 protected:
  static constexpr uint32_t kDeviceId = 1111;
  static constexpr uint32_t kPointerId = 2222;
  // clang-format off
  static constexpr std::array<float, 9> kIdentityMatrix = {
    1, 0, 0, // column one
    0, 1, 0, // column two
    0, 0, 1, // column three
  };
  // clang-format on

  void SetUp() override {
    ScenicCtfTest::SetUp();

    pointerinjector_registry_ = ConnectSyncIntoRealm<fupi::Registry>();

    // Set up root view.
    root_session_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_session_->set_on_close(FailOnClose("Lost connection to Scenic"));

    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    SetFlatlandDisplayContent(std::move(parent_token));

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    root_view_ref_ = scenic::cpp::CloneViewRef(identity.view_ref());
    ASSERT_TRUE((*root_session_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    ASSERT_TRUE((*root_session_)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    ASSERT_TRUE((*root_session_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *root_session_);

    // Get the display's width and height. Since there is no Present in FlatlandDisplay, receiving
    // this callback ensures that all FlatlandDisplay calls are processed.
    fidl::Client<fuc::ParentViewportWatcher> parent_viewport_watcher(
        std::move(parent_viewport_watcher_client_end), dispatcher());
    std::optional<fuc::LayoutInfo> info;
    parent_viewport_watcher->GetLayout().Then(
        [&info](fidl::Result<fuc::ParentViewportWatcher::GetLayout>& result) {
          if (result.is_ok()) {
            info = std::move(result->info());
          }
        });
    RunLoopUntil([&info] { return info.has_value(); });
    display_width_ = static_cast<float>(info->logical_size()->width());
    display_height_ = static_cast<float>(info->logical_size()->height());
  }

  void InjectNewViewport(fupi::Viewport viewport) {
    fupi::Event event;
    event.timestamp(0);
    event.data(fupi::Data::WithViewport(std::move(viewport)));
    std::vector<fupi::Event> events;
    events.emplace_back(std::move(event));
    bool hanging_get_returned = false;
    (*injector_)
        ->Inject({{.events = std::move(events)}})
        .Then([&hanging_get_returned](fidl::Result<fupi::Device::Inject>& result) {
          hanging_get_returned = true;
        });
    RunLoopUntil(
        [this, &hanging_get_returned] { return hanging_get_returned || !injector_->is_bound(); });
  }

  void Inject(float x, float y, fupi::EventPhase phase) {
    fupi::Event event;
    event.timestamp(0);
    {
      fupi::PointerSample pointer_sample;
      pointer_sample.pointer_id(kPointerId);
      pointer_sample.phase(phase);
      pointer_sample.position_in_viewport(std::array<float, 2>{x, y});
      event.data(fupi::Data::WithPointerSample(std::move(pointer_sample)));
    }
    std::vector<fupi::Event> events;
    events.emplace_back(std::move(event));
    bool hanging_get_returned = false;
    (*injector_)
        ->Inject({{.events = std::move(events)}})
        .Then([&hanging_get_returned](fidl::Result<fupi::Device::Inject>& result) {
          hanging_get_returned = true;
        });
    RunLoopUntil(
        [this, &hanging_get_returned] { return hanging_get_returned || !injector_->is_bound(); });
  }

  void RegisterInjector(
      fuv::ViewRef context_view_ref, fuv::ViewRef target_view_ref,
      fupi::DispatchPolicy dispatch_policy = fupi::DispatchPolicy::kExclusiveTarget,
      std::array<float, 9> viewport_to_context_transform = kIdentityMatrix) {
    fupi::Config config;
    config.device_id(kDeviceId);
    config.device_type(fupi::DeviceType::kTouch);
    config.dispatch_policy(dispatch_policy);
    config.context(fupi::Context::WithView(std::move(context_view_ref)));
    config.target(fupi::Target::WithView(std::move(target_view_ref)));
    {
      fupi::Viewport viewport;
      viewport.extents(FullScreenExtents());
      viewport.viewport_to_context_transform(viewport_to_context_transform);
      config.viewport(std::move(viewport));
    }

    auto [injector_client_end, injector_server_end] = fidl::CreateEndpoints<fupi::Device>().value();
    injector_ = std::make_unique<SimpleWatcherClient<fupi::Device>>(std::move(injector_client_end),
                                                                    dispatcher());

    auto result = pointerinjector_registry_->Register(
        {{.config = std::move(config), .injector = std::move(injector_server_end)}});
    ASSERT_TRUE(result.is_ok());
    ASSERT_TRUE(injector_->is_bound());
  }

  // Starts a recursive TouchSource::Watch() loop that collects all received events into
  // |out_events|.
  void StartWatchLoop(SimpleWatcherClient<fup::TouchSource>& touch_source,
                      std::vector<fup::TouchEvent>& out_events,
                      fup::TouchResponseType response_type = fup::TouchResponseType::kMaybe) {
    const size_t index = watch_loops_.size();
    watch_loops_.emplace_back();
    watch_loops_.at(index) = [this, &touch_source, &out_events, response_type,
                              index](std::vector<fup::TouchEvent> events) {
      std::vector<fup::TouchResponse> responses;
      for (auto& event : events) {
        if (event.pointer_sample().has_value()) {
          fup::TouchResponse response;
          response.response_type(response_type);
          responses.emplace_back(std::move(response));
        } else {
          responses.emplace_back(fup::TouchResponse{});
        }
      }
      std::move(events.begin(), events.end(), std::back_inserter(out_events));

      touch_source->Watch({{.responses = std::move(responses)}})
          .Then([this, index](fidl::Result<fup::TouchSource::Watch>& result) {
            if (result.is_ok()) {
              watch_loops_.at(index)(std::move(result->events()));
            }
          });
    };
    touch_source->Watch({{.responses = {}}})
        .Then([this, index](fidl::Result<fup::TouchSource::Watch>& result) {
          if (result.is_ok()) {
            watch_loops_.at(index)(std::move(result->events()));
          }
        });
  }

  void ConnectChildView(FlatlandClientWithEventHandler& flatland,
                        fuv::ViewportCreationToken&& token, fuchsia_math::SizeU size,
                        fuc::TransformId viewport_transform_id,
                        fuc::ContentId viewport_content_id) {
    // Let the client_end die.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    fuc::ViewportProperties properties;
    FX_CHECK(display_width_ > 0 && display_height_ > 0);
    properties.logical_size(size);
    EXPECT_TRUE(flatland->CreateTransform({{.transform_id = viewport_transform_id}}).is_ok());
    EXPECT_TRUE(
        flatland
            ->CreateViewport({{.viewport_id = viewport_content_id,
                               .token = std::move(token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE(flatland
                    ->SetContent({{.transform_id = viewport_transform_id,
                                   .content_id = viewport_content_id}})
                    .is_ok());
    EXPECT_TRUE(flatland
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = viewport_transform_id}})
                    .is_ok());
    BlockingPresent(this, flatland);
  }

  // Injects |points| and checks that the events received in |view_events| match with an offset.
  void InjectionHelper(const std::vector<std::array<float, 2>>& points,
                       const std::vector<fup::TouchEvent>& view_events, float x_offset,
                       float y_offset) {
    if (points.empty())
      return;

    for (size_t i = 0; i < points.size(); ++i) {
      fupi::EventPhase phase;
      if (i == 0) {
        phase = fupi::EventPhase::kAdd;
      } else if (i == points.size() - 1) {
        phase = fupi::EventPhase::kRemove;
      } else {
        phase = fupi::EventPhase::kChange;
      }
      Inject(points[i][0], points[i][1], phase);
    }

    RunLoopUntil([&view_events, num_points = points.size()] {
      // Depending on contest results there may be a TouchInteractionResult appended to
      // |view_events|.
      return view_events.size() >= num_points;
    });  // Succeeds or times out.

    const auto& viewport_to_view_transform =
        view_events[0].view_parameters()->viewport_to_view_transform();

    for (size_t i = 0; i < points.size(); ++i) {
      fup::EventPhase phase;
      if (i == 0) {
        phase = fup::EventPhase::kAdd;
      } else if (i == points.size() - 1) {
        phase = fup::EventPhase::kRemove;
      } else {
        phase = fup::EventPhase::kChange;
      }

      EXPECT_EQ_POINTER(view_events[i].pointer_sample(), viewport_to_view_transform, phase,
                        points[i][0] + x_offset, points[i][1] + y_offset);
    }
  }

  fuchsia_math::SizeU FullScreenSize() const {
    return fuchsia_math::SizeU(static_cast<uint32_t>(display_width_),
                               static_cast<uint32_t>(display_height_));
  }

  std::array<std::array<float, 2>, 2> FullScreenExtents() const {
    return {{{0, 0}, {display_width_, display_height_}}};
  }

  std::unique_ptr<FlatlandClientWithEventHandler> root_session_;
  fuv::ViewRef root_view_ref_;
  float display_width_ = 0;
  float display_height_ = 0;

  std::unique_ptr<SimpleWatcherClient<fupi::Device>> injector_;

 private:
  fidl::SyncClient<fupi::Registry> pointerinjector_registry_;

  // Holds watch loops so they stay alive through the duration of the test.
  std::vector<std::function<void(std::vector<fup::TouchEvent>)>> watch_loops_;
};

TEST_F(FlatlandTouchIntegrationTest, BasicInputTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  Inject(display_width_, display_height_, fupi::EventPhase::kAdd);
  Inject(display_width_, 0, fupi::EventPhase::kChange);
  Inject(0, 0, fupi::EventPhase::kChange);
  Inject(0, display_height_, fupi::EventPhase::kRemove);

  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  // Target should receive identical events to injected, since their coordinate spaces are the same.
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, display_width_, display_height_);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, display_width_, 0.f);
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, 0.f, 0.f);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 0.f, display_height_);
  }
}

TEST_F(FlatlandTouchIntegrationTest, TouchSourceV2_BasicInputTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSourceV2>().value();
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSourceV2 channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source_v2(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  TouchSourceV2Client child_touch_source(std::move(child_touch_source_client_end), dispatcher(),
                                         child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  Inject(display_width_, display_height_, fupi::EventPhase::kAdd);
  Inject(display_width_, 0, fupi::EventPhase::kChange);
  Inject(0, 0, fupi::EventPhase::kChange);
  Inject(0, display_height_, fupi::EventPhase::kRemove);

  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  // Target should receive identical events to injected, since their coordinate spaces are the same.
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, display_width_, display_height_);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, display_width_, 0.f);
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, 0.f, 0.f);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 0.f, display_height_);
  }
}

// With a smaller viewport than the context view, test for two things:
//
// 1) Touches starting *outside* the viewport should miss completely
// 2) Touches starting *inside* the viewport and then leaving the viewport should all be delivered
TEST_F(FlatlandTouchIntegrationTest, ViewportSmallerThanContextView) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  // Set the viewport to only be the top-left quadrant of the screen.
  fupi::Viewport viewport;
  viewport.extents(std::array<std::array<float, 2>, 2>{
      {{0.f, 0.f}, {display_width_ / 2.f, display_height_ / 2.f}}});
  viewport.viewport_to_context_transform(kIdentityMatrix);
  InjectNewViewport(std::move(viewport));

  // Start a touch event stream outside of the viewport. These 4 events should not be received.
  Inject(display_width_, display_height_, fupi::EventPhase::kAdd);
  Inject(0, 0, fupi::EventPhase::kChange);
  Inject(display_width_, 0, fupi::EventPhase::kChange);
  Inject(0, display_height_, fupi::EventPhase::kRemove);

  // Start a touch event stream inside of the viewport, and even the events outside of the viewport
  // should still be delivered.
  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(display_width_, 0, fupi::EventPhase::kChange);
  Inject(display_width_, display_height_, fupi::EventPhase::kChange);
  Inject(0, display_height_, fupi::EventPhase::kRemove);

  // Although 8 events were injected, only the latter 4 should be delivered.
  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  // Target should receive identical events to injected, since their coordinate spaces are the same.
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 0.f, 0.f);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, display_width_, 0.f);
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, display_width_, display_height_);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 0.f, display_height_);
  }
}

TEST_F(FlatlandTouchIntegrationTest, DisconnectTargetView_TriggersChannelClosure) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  // Break the scene graph relation that the pointerinjector relies on. Observe the channel close
  // (lazily).
  EXPECT_TRUE((*child_session)->ReleaseView().is_ok());
  BlockingPresent(this, *child_session);
  BlockingPresent(this, *root_session_);

  // Inject an event to trigger the channel closure.
  Inject(0, 0, fupi::EventPhase::kAdd);
  RunLoopUntil([this] { return !injector_->is_bound(); });  // Succeeds or times out.
}

// In this test we set up the context and the target. We apply a scale, rotation and translation
// transform to both of their viewports, and then inject pointer events to confirm that
// the coordinates received by the listener are correctly transformed.
// Only the transformation of the target, relative to the context, should have any effect on
// the output.
// The viewport-to-context transform here is the identity. That is, the size of the 9x9 viewport
// matches the size of the 5x5 context view.
//
// Below are ASCII diagrams showing the transformation *difference* between target and context.
// Note that the dashes represent the context view and notated X,Y coordinate system is the
// context's coordinate system. The target view's coordinate system has its origin at corner '1'.
//
// Scene pre-transformation
// 1,2,3,4 denote the corners of the target view:
//   X ->
// Y 1 O O O O 2
// | O O O O O O
// v O O O O O O
//   O O O O O O
//   O O O O O O
//   4 O O O O 3
//
// After scale:
//   X ->
// Y 1 - O - O - O   O   2
// | - - - - - - -
// V - - - - - - -
//   O - O - O - O   O   O
//   - - - - - - -
//   - - - - - - -
//   O   O   O   O   O   O
//
//
//   O   O   O   O   O   O
//
//
//   O   O   O   O   O   O
//
//
//   4   O   O   O   O   3
//
// After rotation:
//   X ->
// Y 4      O      O      O      O      1 - - - - - -
// |                                      - - - - - -
// V O      O      O      O      O      O - - - - - -
//                                        - - - - - -
//   O      O      O      O      O      O - - - - - -
//                                        - - - - - -
//   O      O      O      O      O      O
//
//   O      O      O      O      O      O
//
//   3      O      O      O      O      2
//
// After translation:
//   X ->
// Y 4      O      O      O      O    A 1 - - - C1
// |                                  - - - - - -
// V O      O      O      O      O    - O - - - -
//                                    - - - - - -
//   O      O      O      O      O    - O - - - -
//                                    R - - - - C2
//   O      O      O      O      O      O
//
//   O      O      O      O      O      O
//
//   3      O      O      O      O      2
TEST_F(FlatlandTouchIntegrationTest, TargetViewWithScaleRotationTranslation) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Scale, rotate, and translate the child_session. Those operations are applied in that order.
  EXPECT_TRUE((*root_session_)
                  ->SetScale({{.transform_id = kViewportTransform, .scale = VecF(2, 3)}})
                  .is_ok());
  EXPECT_TRUE((*root_session_)
                  ->SetOrientation({{.transform_id = kViewportTransform,
                                     .orientation = Orientation::kCcw270Degrees}})
                  .is_ok());
  EXPECT_TRUE((*root_session_)
                  ->SetTranslation({{.transform_id = kViewportTransform, .translation = Vec(1, 0)}})
                  .is_ok());
  BlockingPresent(this, *root_session_);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(10, 0, fupi::EventPhase::kChange);
  Inject(0, 10, fupi::EventPhase::kChange);
  Inject(10, 10, fupi::EventPhase::kRemove);

  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  // For a CCW_270 rotation, the new x' and y' from x and y is:
  // x' = y
  // y' = -x
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 0.f / 2.f, (0.f + 1.f) / 3.f);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, 0.f / 2.f, (-10.f + 1.f) / 3.f);
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kChange, 10.f / 2.f, (0.f + 1.f) / 3.f);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 10.f / 2.f, (-10.f + 1.f) / 3.f);
  }
}

// Create a 10x10 root view, and 10x10 child view.
//
// Rotate the child 90 degrees and ensure that touches starting on each corner get delivered. This
// confirms that small floating point deviations don't cause issues.
TEST_F(FlatlandTouchIntegrationTest, InjectedInput_OnRotatedChild_ShouldHitEdges) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Rotate the transform holding the child session and then translate it back into position.
  EXPECT_TRUE((*root_session_)
                  ->SetOrientation({{.transform_id = kViewportTransform,
                                     .orientation = Orientation::kCcw270Degrees}})
                  .is_ok());
  EXPECT_TRUE(
      (*root_session_)
          ->SetTranslation({{.transform_id = kViewportTransform, .translation = Vec(10, 0)}})
          .is_ok());

  {
    // Clip the root session.
    Rect rect(0, 0, 10, 10);
    EXPECT_TRUE((*root_session_)
                    ->SetClipBoundary(
                        {{.transform_id = kRootTransform, .rect = std::make_unique<Rect>(rect)}})
                    .is_ok());
  }
  {
    // Clip the child session.
    Rect rect(0, 0, 10, 10);
    EXPECT_TRUE((*root_session_)
                    ->SetClipBoundary({{.transform_id = kViewportTransform,
                                        .rect = std::make_unique<Rect>(rect)}})
                    .is_ok());
  }

  BlockingPresent(this, *root_session_);
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kTopHitAndAncestorsInTarget);

  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(0, 0, fupi::EventPhase::kRemove);

  Inject(10, 0, fupi::EventPhase::kAdd);
  Inject(10, 0, fupi::EventPhase::kRemove);

  Inject(0, 10, fupi::EventPhase::kAdd);
  Inject(0, 10, fupi::EventPhase::kRemove);

  Inject(10, 10, fupi::EventPhase::kAdd);
  Inject(10, 10, fupi::EventPhase::kRemove);

  RunLoopUntil([&child_events] { return child_events.size() == 8u; });  // Succeeds or times out.

  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 0.f, 10.f);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 0.f, 10.f);

    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 0.f, 0.f);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 0.f, 0.f);

    EXPECT_EQ_POINTER(child_events[4].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 10.f, 10.f);
    EXPECT_EQ_POINTER(child_events[5].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 10.f, 10.f);

    EXPECT_EQ_POINTER(child_events[6].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kAdd, 10.f, 0.f);
    EXPECT_EQ_POINTER(child_events[7].pointer_sample(), viewport_to_view_transform,
                      EventPhase::kRemove, 10.f, 0.f);
  }
}

// This test creates a view tree of the form:
//
//    root_view
//        |
//   parent_view
//     /      \
// child_A  child_B
//
// Where the root and parent are full screen, and child_A and child_B are partial screen views with
// some overlap in the middle. Child A is "A"bove child B, who is "B"elow.
//
//
// Let width and height represent the display width and height. Consider the following diagram,
// drawn mostly to scale.
//
// There are two full screen views: root (context) and parent (target). For event streams 1, 2, and
// 4, the viewport is full screen. For #4 the viewport is full screen but scaled, see that part of
// the code for more details.
//
// The top-left point of A is (width / 4, height / 4). The top-left point of
// B is (width/2, height/4).Both A and B have the same dimensions: [width / 2 x height x 2].
// Partial screen views: A on the left, B on the right, with A and B overlapping in the middle.
// -------------------------------------
// |Root/Parent/Viewport               |
// |                                   |
// |        ---------------------------|
// |        |A       | <AB>   |       B|
// |        |        |        |        |
// |        |        |        |        |
// |        ---------------------------|
// |                                   |
// |                                   |
// -------------------------------------
//
//
// Diagram for event stream #3 with a transformed viewport (VP):
// Top-left point of the viewport is the same as A's top-left point. Bottom-right point of the
// viewport is context view's bottom-right point.
// -------------------------------------
// |Root/Parent                        |
// |                                   |
// |        ---------------------------|
// |        |A/VP    | <AB>/VP|    B/VP|
// |        |        |        |        |
// |        |        |        |        |
// |        |- - - - - - - - - - - - - |
// |        |                          |
// |        |                  Viewport|
// -------------------------------------
TEST_F(FlatlandTouchIntegrationTest, PartialScreenOverlappingViews) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view and attach it to |root_session_|. Register the parent view to receive
  // input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(
        *root_session_, std::move(parent_token),
        SizeU(static_cast<uint32_t>(display_width_), static_cast<uint32_t>(display_height_)),
        kViewportTransform, kRootContentId);

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    // The parent's Present call generates a snapshot which includes the ViewRef.
    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }

  auto [child_token_A, parent_token_A] = scenic::cpp::ViewCreationTokenPair::New();
  auto [child_token_B, parent_token_B] = scenic::cpp::ViewCreationTokenPair::New();
  TransformId kTransformId_A(2);
  TransformId kTransformId_B(3);
  ContentId kContent_A(2);
  ContentId kContent_B(3);

  // Create child view A.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session_A;
  auto [child_A_touch_source_client_end, child_A_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_A_touch_source(
      std::move(child_A_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session_A = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Create child view B.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session_B;
  auto [child_B_touch_source_client_end, child_B_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_B_touch_source(
      std::move(child_B_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session_B = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Define A and B width and height as half of the display width and height.
  uint32_t half_width = static_cast<int32_t>(display_width_) / 2;
  uint32_t half_height = static_cast<int32_t>(display_height_) / 2;

  // "A" should be connected after "B", since the topologically-last view is highest in paint order,
  // and therefore above its sibling views.
  ConnectChildView(*parent_session, std::move(parent_token_B), SizeU(half_width, half_height),
                   kTransformId_B, kContent_B);
  ConnectChildView(*parent_session, std::move(parent_token_A), SizeU(half_width, half_height),
                   kTransformId_A, kContent_A);

  // Set up child view A.
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_A_touch_source_server_end));
    EXPECT_TRUE((*child_session_A)
                    ->CreateView2({{.token = std::move(child_token_A),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session_A)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session_A)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  }

  // Set up child view B.
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_B_touch_source_server_end));
    EXPECT_TRUE((*child_session_B)
                    ->CreateView2({{.token = std::move(child_token_B),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session_B)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session_B)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  }

  // A starts at 1/4 the width of the screen, and goes until the 3/4 mark.
  int32_t view_A_x = static_cast<int32_t>(display_width_) / 4;
  int32_t view_A_width = static_cast<int32_t>(half_width);
  int32_t view_A_y = static_cast<int32_t>(display_height_) / 4;
  int32_t view_A_height = static_cast<int32_t>(half_height);

  // B starts at 1/2 the width of the screen, and goes until the 4/4 mark.
  //
  // This implies that there is overlap between the 1/2 and 3/4 marks of the screen.
  int32_t view_B_x = static_cast<int32_t>(display_width_) / 2;
  int32_t view_B_width = static_cast<int32_t>(half_width);
  int32_t view_B_y = static_cast<int32_t>(display_height_) / 4;
  int32_t view_B_height = static_cast<int32_t>(half_height);

  // Define all useful coords for convenience later.
  float A_x_min = static_cast<float>(view_A_x);
  float A_x_max = static_cast<float>(view_A_x + view_A_width);
  float A_y_min = static_cast<float>(view_A_y);
  float A_y_max = static_cast<float>(view_A_y + view_A_height);

  float B_x_min = static_cast<float>(view_B_x);
  float B_x_max = static_cast<float>(view_B_x + view_B_width);
  float B_y_min = static_cast<float>(view_B_y);
  float B_y_max = static_cast<float>(view_B_y + view_B_height);

  float A_B_height = static_cast<float>(view_A_height);
  float A_B_combined_width = B_x_max - A_x_min;

  // Ensure there's overlap with A and B.
  EXPECT_TRUE(A_x_min <= B_x_min && B_x_min <= A_x_max && A_x_max <= B_x_max);
  EXPECT_TRUE(A_y_min == B_y_min && A_y_max == B_y_max);

  EXPECT_TRUE((*parent_session)
                  ->SetTranslation(
                      {{.transform_id = kTransformId_A, .translation = Vec(view_A_x, view_A_y)}})
                  .is_ok());
  EXPECT_TRUE((*parent_session)
                  ->SetTranslation(
                      {{.transform_id = kTransformId_B, .translation = Vec(view_B_x, view_B_y)}})
                  .is_ok());

  // Commit all changes.
  BlockingPresent(this, *parent_session);
  BlockingPresent(this, *child_session_A);
  BlockingPresent(this, *child_session_B);

  // Listen for input events.
  std::vector<TouchEvent> child_A_events;
  StartWatchLoop(child_A_touch_source, child_A_events);

  std::vector<TouchEvent> child_B_events;
  StartWatchLoop(child_B_touch_source, child_B_events);

  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events);

  /***** Setup done. Begin injecting input events into the scene. *****/

  /*
   * Event stream #1.
   */

  // Start a touch event stream in the middle of the screen, where A and B overlap. A should receive
  // the input events even as it goes from A to B and vice-versa.

  std::vector<std::array<float, 2>> points;
  points.reserve(5);

  points.push_back({B_x_min, B_y_min});
  points.push_back({A_x_min, A_y_min});
  points.push_back({B_x_max, B_y_max});
  points.push_back({B_x_max, B_y_min});
  points.push_back({A_x_min, A_y_max});

  // Translate all expected points by [-A_x_min, -A_y_min] since the viewport_to_view_transform
  // transforms points into A's coordinate space.
  InjectionHelper(points, child_A_events, -A_x_min, -A_y_min);

  // Ensure parent also received events, but not the below sibling.
  RunLoopUntil([&parent_events] {
    return !parent_events.empty() && parent_events.back().interaction_result().has_value();
  });
  RunLoopUntil([&child_A_events] {
    return !child_A_events.empty() && child_A_events.back().interaction_result().has_value();
  });
  EXPECT_EQ(parent_events.size(), 6u);   // 5 events + TouchInteractionResult
  EXPECT_EQ(child_A_events.size(), 6u);  // 5 events + TouchInteractionResult
  EXPECT_EQ(child_B_events.size(), 0u);

  // Reset vectors for the next stream.
  parent_events.clear();
  child_A_events.clear();
  child_B_events.clear();
  points.clear();

  /*
   * Event stream #2.
   */

  // Start a touch event stream over B. B should receive the input events even as it goes over A.

  points.reserve(5);
  points.push_back({B_x_max, B_y_max});
  points.push_back({A_x_min, A_y_min});
  points.push_back({B_x_min, B_y_min});
  points.push_back({B_x_max, B_y_min});
  points.push_back({A_x_min, A_y_max});

  InjectionHelper(points, child_B_events, -B_x_min, -B_y_min);

  // Ensure parent also received events, but not the above sibling.
  RunLoopUntil([&parent_events] {
    return !parent_events.empty() && parent_events.back().interaction_result().has_value();
  });
  RunLoopUntil([&child_B_events] {
    return !child_B_events.empty() && child_B_events.back().interaction_result().has_value();
  });
  EXPECT_EQ(parent_events.size(), 6u);   // 5 events + TouchInteractionResult
  EXPECT_EQ(child_B_events.size(), 6u);  // 5 events + TouchInteractionResult
  EXPECT_EQ(child_A_events.size(), 0u);

  // Reset vectors for the next stream.
  parent_events.clear();
  child_A_events.clear();
  child_B_events.clear();
  points.clear();

  /*
   * Event stream #3.
   */

  // Change the viewport size and translate it.

  // Keep the bottom-right corner of the viewport the same, and move the top-left corner to be equal
  // to view A's top-left corner.
  {
    fupi::Viewport viewport;
    viewport.extents(std::array<std::array<float, 2>, 2>{
        {{0.f, 0.f}, {display_width_ - A_x_min, display_height_ - A_y_min}}});
    viewport.viewport_to_context_transform(std::array<float, 9>{1, 0, 0,                // col 1
                                                                0, 1, 0,                // col 2
                                                                A_x_min, A_y_min, 1});  // col 3
    InjectNewViewport(std::move(viewport));
  }

  points.push_back({0, 0});
  points.push_back({A_B_combined_width, 0});
  points.push_back({A_B_combined_width, A_B_height});
  points.push_back({0, A_B_height});

  InjectionHelper(points, child_A_events, 0, 0);
  RunLoopUntil([&child_A_events] {
    return !child_A_events.empty() && child_A_events.back().interaction_result().has_value();
  });
  RunLoopUntil([&parent_events] {
    return !parent_events.empty() && parent_events.back().interaction_result().has_value();
  });
  EXPECT_EQ(child_A_events.size(), 5u);
  EXPECT_EQ(parent_events.size(), 5u);
  EXPECT_TRUE(child_B_events.empty());

  // Reset vectors for the next stream.
  parent_events.clear();
  child_A_events.clear();
  child_B_events.clear();
  points.clear();

  /*
   * Event stream #4.
   */

  // Scale the viewport to be the same size as the context view but with double the "resolution".
  // Meaning a point at (x,y) in the context coordinate space is at (2x,2y) in the viewport
  // coordinate space.

  {
    fupi::Viewport viewport;
    viewport.extents(std::array<std::array<float, 2>, 2>{
        {{0.f, 0.f}, {display_width_ * 2.f, display_height_ * 2.f}}});
    viewport.viewport_to_context_transform(std::array<float, 9>{0.5, 0, 0,  // col 1
                                                                0, 0.5, 0,  // col 2
                                                                0, 0, 1});  // col 3
    InjectNewViewport(std::move(viewport));
  }

  // Injecting a touch at (A_x_max * 2, A_y_max * 2) should actually hit A at its bottom right
  // corner, given the viewport scale changes.
  points.push_back({A_x_max, A_y_max});
  points.push_back({A_x_max, A_y_min});
  points.push_back({A_x_min, A_y_min});
  points.push_back({A_x_min, A_y_max});

  for (size_t i = 0; i < points.size(); ++i) {
    fupi::EventPhase phase;
    if (i == 0) {
      phase = fupi::EventPhase::kAdd;
    } else if (i == points.size() - 1) {
      phase = fupi::EventPhase::kRemove;
    } else {
      phase = fupi::EventPhase::kChange;
    }
    Inject(points[i][0] * 2, points[i][1] * 2, phase);
  }

  RunLoopUntil([&child_A_events] {
    // 4 events + TouchInteractionResult.
    return child_A_events.size() == 5u;
  });  // Succeeds or times out.

  // Offset |points| by A's top-left point.
  for (auto& point : points) {
    point[0] -= A_x_min;
    point[1] -= A_y_min;
  }

  const auto& viewport_to_view_transform =
      child_A_events[0].view_parameters()->viewport_to_view_transform();
  for (size_t i = 0; i < points.size(); ++i) {
    EventPhase phase = EventPhase::kChange;
    if (i == 0) {
      phase = EventPhase::kAdd;
    } else if (i == points.size() - 1) {
      phase = EventPhase::kRemove;
    }

    EXPECT_EQ_POINTER(child_A_events[i].pointer_sample(), viewport_to_view_transform, phase,
                      points[i][0], points[i][1]);
  }
}

// Creates a view tree of the form
// root_view
//    |
// parent_view
//    |
// child_view
// The parent's view gets created using CreateView2 but the child's view gets created using
// CreateView. As a result, the child will not receive any input events since it does not have an
// associated ViewRef.
TEST_F(FlatlandTouchIntegrationTest, ChildCreatedUsingCreateView_DoesNotGetInput) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view using CreateView2 and attach it to |root_session_|. Register the parent
  // view to receive input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                     kRootContentId);

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    // The parent's Present call generates a snapshot which includes the ViewRef.
    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }

  // Create the child view using CreateView and attach it to |parent_session|.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    ConnectChildView(*parent_session, std::move(parent_token), FullScreenSize(), kViewportTransform,
                     kRootContentId);

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*child_session)
                    ->CreateView({{.token = std::move(child_token),
                                   .parent_viewport_watcher =
                                       std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    // The child's Present call generates a snapshot which will not include a ViewRef.
    BlockingPresent(this, *child_session);
  }

  // Listen for input events.
  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events);
  // (0,0) is the origin. The child and the parent both overlap at the origin so they both are
  // eligible to receive the input event at this point.
  Inject(0, 0, fupi::EventPhase::kAdd);
  RunLoopUntilIdle();

  // |parent_session| receives the input event.
  EXPECT_EQ(parent_events.size(), 1u);
}

// Creates a view tree of the form
// root_view
//    |
// parent_view
//    |
// child_view
// Parent and child each have a view ref, and the parent positions its hit region and its child like
// so:
// ------------------------------------
// |                |                 |
// |       A        |        B        |
// |                |                 |
// |----------------------------------|
// |                |                 |
// |       C        |        D        |
// |                |                 |
// |----------------------------------|
// where parent view occupies A, B, C, D,
// and child view occupies A and C (the left-side half of the parent view),
// and the parent view positions a hit region over C and D, which *covers* the child view.
//
// The hit tests expected are:
// - hit test in A should reach the child, due to the default hit region for the child.
// - hit test in B should reach the parent, due to the default hit region for the parent.
// - hit test in C and D should reach the parent, due to the parent's explicit hit region.
TEST_F(FlatlandTouchIntegrationTest, SetHitRegion_CapturesTouch) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view and attach it to |root_session_|. Register for input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                     kRootContentId);

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the child view and attach it to the |parent_session|. Register for input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    const SizeU kHalfWidthViewSize(static_cast<uint32_t>(display_width_ / 2),
                                   static_cast<uint32_t>(display_height_));
    ConnectChildView(*parent_session, std::move(parent_token), kHalfWidthViewSize,
                     kViewportTransform, kRootContentId);

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    BlockingPresent(this, *child_session);
  }

  // Finally, parent view adds its hit region on top of the child view.
  {
    TransformId kTransformId(3);
    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kTransformId}}).is_ok());
    RectF region(0, display_height_ / 2, display_width_, display_height_ / 2);
    EXPECT_TRUE(
        (*parent_session)
            ->SetHitRegions({{.transform_id = kTransformId,
                              .regions = {HitRegion(region, HitTestInteraction::kDefault)}}})
            .is_ok());
    EXPECT_TRUE((*parent_session)
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = kTransformId}})
                    .is_ok());
    BlockingPresent(this, *parent_session);
  }

  // Listen for input events.
  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events);
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  const float x_1q = display_width_ * 1 / 4;
  const float x_3q = display_width_ * 3 / 4;
  const float y_1q = display_height_ * 1 / 4;
  const float y_3q = display_height_ * 3 / 4;

  Inject(x_1q, y_1q, fupi::EventPhase::kAdd);     // quadrant A
  Inject(x_1q, y_1q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly

  // Drive loop until all expected events come in.
  RunLoopUntil([&parent_events, &child_events] {
    return parent_events.size() >= 3u && child_events.size() >= 3u;
  });

  // One tap, both parent and child get two coordinate samples, plus a final interaction result that
  // occurs only on gesture competition. We ignore the interaction result, as this test focuses on
  // coordinates.
  ASSERT_EQ(parent_events.size(), 3u);
  ASSERT_EQ(child_events.size(), 3u);
  {
    const auto& vtvt = child_events[0].view_parameters()->viewport_to_view_transform();
    // Child view has half width of display. Pointer event is in middle of that view.
    const float child_x = display_width_ / 4;
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), vtvt, EventPhase::kAdd, child_x, y_1q);
  }

  Inject(x_3q, y_1q, fupi::EventPhase::kAdd);     // quadrant B
  Inject(x_3q, y_1q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  RunLoopUntilIdle();
  // Another tap, two more samples to just the parent. No competition, so the interaction result is
  // immediate and dispatched with the samples.
  ASSERT_EQ(parent_events.size(), 5u);
  ASSERT_EQ(child_events.size(), 3u);
  {
    const auto& vtvt = parent_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(parent_events[3].pointer_sample(), vtvt, EventPhase::kAdd, x_3q, y_1q);
  }

  Inject(x_1q, y_3q, fupi::EventPhase::kAdd);     // quadrant C
  Inject(x_1q, y_3q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  Inject(x_3q, y_3q, fupi::EventPhase::kAdd);     // quadrant D
  Inject(x_3q, y_3q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  RunLoopUntilIdle();
  // Two more taps, each with two samples: four more samples to just the parent. No competition, so
  // the interaction result is immediate and dispatched with the samples.
  ASSERT_EQ(parent_events.size(), 9u);
  ASSERT_EQ(child_events.size(), 3u);
  {
    const auto& vtvt = parent_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(parent_events[5].pointer_sample(), vtvt, EventPhase::kAdd, x_1q, y_3q);
    EXPECT_EQ_POINTER(parent_events[7].pointer_sample(), vtvt, EventPhase::kAdd, x_3q, y_3q);
  }
}

// Creates a view tree of the form
// root_view
//    |
// parent_view
//    |
// child_view
// Parent and child each have a view ref, and the parent positions its hit region and its child like
// so:
// ------------------------------------
// |                |                 |
// |       A        |        B        |
// |                |                 |
// |----------------------------------|
// |                |                 |
// |       C        |        D        |
// |                |                 |
// |----------------------------------|
// where parent view occupies A, B, C, D,
// and child view occupies A and C (the left-side half of the parent view),
// and the parent view positions an infinite hit region that *covers* the child view.
//
// Then a hit test in every sector (A, B, C, D) should reach the parent.
TEST_F(FlatlandTouchIntegrationTest, InfiniteHitRegion_CapturesTouch) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view and attach it to |root_session_|. Register for input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                     kRootContentId);

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }

  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the child view and attach it to the |parent_session|. Register for input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    child_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());

    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    const SizeU kHalfWidthViewSize(static_cast<uint32_t>(display_width_ / 2),
                                   static_cast<uint32_t>(display_height_));
    ConnectChildView(*parent_session, std::move(parent_token), kHalfWidthViewSize,
                     kViewportTransform, kRootContentId);

    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    BlockingPresent(this, *child_session);
  }

  // Finally, parent view adds its infinite hit region on top of the child view.
  {
    TransformId kTransformId(3);
    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kTransformId}}).is_ok());
    EXPECT_TRUE((*parent_session)
                    ->SetInfiniteHitRegion(
                        {{.transform_id = kTransformId, .hit_test = HitTestInteraction::kDefault}})
                    .is_ok());
    EXPECT_TRUE((*parent_session)
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = kTransformId}})
                    .is_ok());
    BlockingPresent(this, *parent_session);
  }

  // Listen for input events.
  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events);
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  const float x_1q = display_width_ * 1 / 4;
  const float x_3q = display_width_ * 3 / 4;
  const float y_1q = display_height_ * 1 / 4;
  const float y_3q = display_height_ * 3 / 4;

  Inject(x_1q, y_1q, fupi::EventPhase::kAdd);     // quadrant A
  Inject(x_1q, y_1q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  Inject(x_3q, y_1q, fupi::EventPhase::kAdd);     // quadrant B
  Inject(x_3q, y_1q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  Inject(x_1q, y_3q, fupi::EventPhase::kAdd);     // quadrant C
  Inject(x_1q, y_3q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  Inject(x_3q, y_3q, fupi::EventPhase::kAdd);     // quadrant D
  Inject(x_3q, y_3q, fupi::EventPhase::kRemove);  // ends pointer state machine cleanly
  RunLoopUntilIdle();
  ASSERT_EQ(parent_events.size(), 8u);  // No competition with child, so the interaction result is
  ASSERT_EQ(child_events.size(), 0u);   // immediate and dispatched with the samples.
  {
    const auto& vtvt = parent_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(parent_events[0].pointer_sample(), vtvt, EventPhase::kAdd, x_1q, y_1q);
    EXPECT_EQ_POINTER(parent_events[2].pointer_sample(), vtvt, EventPhase::kAdd, x_3q, y_1q);
    EXPECT_EQ_POINTER(parent_events[4].pointer_sample(), vtvt, EventPhase::kAdd, x_1q, y_3q);
    EXPECT_EQ_POINTER(parent_events[6].pointer_sample(), vtvt, EventPhase::kAdd, x_3q, y_3q);
  }
}

TEST_F(FlatlandTouchIntegrationTest, ExclusiveMode_TargetDisconnectedMidStream_ShouldCancelStream) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref), DispatchPolicy::kExclusiveTarget);

  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(4, 2, fupi::EventPhase::kChange);
  RunLoopUntil([&child_events] { return child_events.size() == 2u; });  // Succeeds or times out.

  EXPECT_TRUE((*root_session_)
                  ->RemoveChild({{.parent_transform_id = kRootTransform,
                                  .child_transform_id = kViewportTransform}})
                  .is_ok());
  BlockingPresent(this, *root_session_);

  // Next event should deliver a cancel event to the child (and close the injector since it's the
  // target)
  Inject(5, 5, fupi::EventPhase::kChange);

  RunLoopUntil([&child_events] { return child_events.size() == 3u; });  // Succeeds or times out.

  EXPECT_TRUE(!injector_->is_bound());
  EXPECT_EQ(child_events.back().pointer_sample()->phase(), EventPhase::kCancel);
}

TEST_F(FlatlandTouchIntegrationTest, ExclusiveMode_TargetDyingMidStream_ShouldKillChannel) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the root graph.
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ConnectChildView(*root_session_, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kRootContentId);

  // Set up the child view and its TouchSource channel.
  auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
      fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
  auto identity = scenic::cpp::NewViewIdentityOnCreation();
  auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
  ViewBoundProtocols protocols;
  protocols.touch_source(std::move(child_touch_source_server_end));
  EXPECT_TRUE((*child_session)
                  ->CreateView2(
                      {{.token = std::move(child_token),
                        .view_identity = std::move(identity),
                        .protocols = std::move(protocols),
                        .parent_viewport_watcher = std::move(parent_viewport_watcher_server_end)}})
                  .is_ok());
  const TransformId kTransform(42);
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kTransform}}).is_ok());
  EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kTransform}}).is_ok());
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref), DispatchPolicy::kExclusiveTarget);

  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(4, 2, fupi::EventPhase::kChange);
  RunLoopUntil([&child_events] { return child_events.size() == 2u; });  // Succeeds or times out.

  // Kill the target.
  EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = TransformId(0)}}).is_ok());
  EXPECT_TRUE((*child_session)->Present({}).is_ok());
  RunLoopUntil([&child_session] { return !child_session->is_bound(); });

  // TODO(https://fxbug.dev/42061828): Present on the root session to flush the changes.
  BlockingPresent(this, *root_session_);

  // Next event should deliver a cancel event to the child (and close the injector since it's the
  // target)
  Inject(5, 5, fupi::EventPhase::kChange);
  RunLoopUntil([this] { return !injector_->is_bound(); });
  EXPECT_TRUE(!injector_->is_bound());
}

// Construct a scene with the following topology:
//
// Root
//   |
// Parent
//   |
// Child
//
// Injects in HitTest mode, all events delivered to Parent and Child. Then, disconnect Child and
// observe contest loss from Child.
TEST_F(FlatlandTouchIntegrationTest, HitTested_ViewDisconnectedMidContest_ShouldLoseContest) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view and attach it to |root_session_|. Register the parent view to receive
  // input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(
        *root_session_, std::move(parent_token),
        SizeU(static_cast<uint32_t>(display_width_), static_cast<uint32_t>(display_height_)),
        kViewportTransform, kRootContentId);

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    // The parent's Present call generates a snapshot which includes the ViewRef.
    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ContentId kContent(2);

  // Create child view.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  ConnectChildView(*parent_session, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kContent);

  // Set up child view.
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_touch_source_server_end));
    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  }

  // Commit all changes.
  BlockingPresent(this, *root_session_);
  BlockingPresent(this, *parent_session);
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events);
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Begin injection - both child and parent should receive it.
  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(1, 1, fupi::EventPhase::kChange);

  // Succeeds or times out.
  RunLoopUntil([&child_events, &parent_events] {
    return child_events.size() == 2u && parent_events.size() == 2u;
  });

  // Disconnect |child_session| and observe that it gets a cancellation event, while
  // |parent_session| keeps receiving events and receives a GRANTED interaction result.
  EXPECT_TRUE((*parent_session)
                  ->RemoveChild({{.parent_transform_id = kRootTransform,
                                  .child_transform_id = kViewportTransform}})
                  .is_ok());
  BlockingPresent(this, *parent_session);

  Inject(2, 2, fupi::EventPhase::kChange);
  Inject(3, 3, fupi::EventPhase::kChange);

  // Succeeds or times out.
  RunLoopUntil([&child_events, &parent_events] {
    return child_events.size() == 3u && parent_events.size() == 5u;
  });

  ASSERT_TRUE(child_events.back().interaction_result().has_value());
  EXPECT_EQ(child_events.back().interaction_result()->status(), TouchInteractionStatus::kDenied);

  EXPECT_TRUE(std::ranges::any_of(parent_events, [](const TouchEvent& event) {
    return event.interaction_result().has_value() &&
           event.interaction_result()->status() == TouchInteractionStatus::kGranted;
  }));
}

// Construct a scene with the following topology:
//
// Root
//   |
// Parent
//   |
// Child
//
// Injects in HitTest mode, all events delivered to Parent and Child. Parent replies "NO" to its
// events, so Child wins the contest. Then, disconnect child disconnect Child and observe cancel
// event delivered to Child.
TEST_F(FlatlandTouchIntegrationTest, HitTested_ViewDisconnectedAfterWinning_ShouldCancelStream) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_session;
  auto [parent_touch_source_client_end, parent_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> parent_touch_source(
      std::move(parent_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));

  // Create the parent view and attach it to |root_session_|. Register the parent view to receive
  // input events.
  {
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    parent_session = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(parent_touch_source_server_end));
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ConnectChildView(
        *root_session_, std::move(parent_token),
        SizeU(static_cast<uint32_t>(display_width_), static_cast<uint32_t>(display_height_)),
        kViewportTransform, kRootContentId);

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());

    // The parent's Present call generates a snapshot which includes the ViewRef.
    BlockingPresent(this, *parent_session);
    RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                     scenic::cpp::CloneViewRef(parent_view_ref),
                     DispatchPolicy::kTopHitAndAncestorsInTarget);
  }
  auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
  ContentId kContent(2);

  // Create child view A.
  std::unique_ptr<FlatlandClientWithEventHandler> child_session;
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  ConnectChildView(*parent_session, std::move(parent_token), FullScreenSize(), kViewportTransform,
                   kContent);

  // Set up child view A.
  {
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_touch_source_server_end));
    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
  }

  // Commit all changes.
  BlockingPresent(this, *root_session_);
  BlockingPresent(this, *parent_session);
  BlockingPresent(this, *child_session);

  // Listen for input events.
  std::vector<TouchEvent> parent_events;
  StartWatchLoop(parent_touch_source, parent_events, TouchResponseType::kNo);
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Begin injection.
  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(5, 0, fupi::EventPhase::kChange);

  // Child should win the contest.
  RunLoopUntil([&child_events] { return child_events.size() == 3u; });  // Succeeds or times out.
  ASSERT_EQ(child_events.size(), 3u);
  EXPECT_TRUE(std::ranges::any_of(child_events, [](const TouchEvent& event) {
    return event.interaction_result().has_value() &&
           event.interaction_result()->status() == TouchInteractionStatus::kGranted;
  }));

  // Detach child_session from the scene graph.
  EXPECT_TRUE((*parent_session)
                  ->RemoveChild({{.parent_transform_id = kRootTransform,
                                  .child_transform_id = kViewportTransform}})
                  .is_ok());
  BlockingPresent(this, *parent_session);

  // Next event should deliver CANCEL to Child.
  Inject(5, 5, fupi::EventPhase::kChange);
  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.
  ASSERT_EQ(child_events.size(), 4u);
  ASSERT_TRUE(child_events.back().pointer_sample().has_value());
  ASSERT_TRUE(child_events.back().pointer_sample()->phase().has_value());
  EXPECT_EQ(child_events.back().pointer_sample()->phase(), EventPhase::kCancel);

  // Future injections should be ignored.
  parent_events.clear();
  child_events.clear();
  Inject(0, 5, fupi::EventPhase::kChange);
  EXPECT_TRUE(parent_events.empty());
  EXPECT_TRUE(child_events.empty());
}

// This test mimics how a11y implements magnification, injects tap events and checks if the pointer
// events are transformed correctly. The test exercises the following view topology:-
//  root_view
//      |
//  parent_view(a11y view)
//      |
//  child_view
// The parent view acts as the a11y view and applies scale and translation to the child view to
// mimic magnification.
TEST_F(FlatlandTouchIntegrationTest, MagnificationTest) {
  // Set up the parent_view and connect it to the root_view.
  auto parent_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  ViewRef parent_view_ref;
  {
    auto [view_token, viewport_creation_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    parent_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    EXPECT_TRUE((*parent_session)
                    ->CreateView2({{.token = std::move(view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    EXPECT_TRUE((*parent_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*parent_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    ConnectChildView(*root_session_, std::move(viewport_creation_token), FullScreenSize(),
                     kViewportTransform, kRootContentId);
  }

  // Set up the child_view and connect it to the parent_view.
  auto child_session = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());

  // Set up the touch source channel for the child_view so that it can listen to the pointer events.
  auto [child_touch_source_client_end, child_touch_source_server_end] =
      fidl::CreateEndpoints<fup::TouchSource>().value();
  SimpleWatcherClient<fup::TouchSource> child_touch_source(
      std::move(child_touch_source_client_end), dispatcher(), FailOnClose("Touch source closed"));
  ViewRef child_view_ref;

  {
    auto [view_token, viewport_creation_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();

    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());

    ViewBoundProtocols protocols;
    protocols.touch_source(std::move(child_touch_source_server_end));

    EXPECT_TRUE((*child_session)
                    ->CreateView2({{.token = std::move(view_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_session)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_session)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *child_session);

    ConnectChildView(*parent_session, std::move(viewport_creation_token), FullScreenSize(),
                     kViewportTransform, kRootContentId);
  }

  // parent_view applies scale and translation to the child_view to mimic magnification.
  const float scale_x = 2.f, scale_y = 2.f;
  const int32_t translation_x = static_cast<int32_t>(display_width_) / 2,
                translation_y = static_cast<int32_t>(display_height_) / 2;
  EXPECT_TRUE(
      (*parent_session)
          ->SetScale({{.transform_id = kViewportTransform, .scale = VecF(scale_x, scale_y)}})
          .is_ok());
  EXPECT_TRUE((*parent_session)
                  ->SetTranslation({{.transform_id = kViewportTransform,
                                     .translation = Vec(translation_x, translation_y)}})
                  .is_ok());
  BlockingPresent(this, *parent_session);

  // Listen for input events.
  std::vector<TouchEvent> child_events;
  StartWatchLoop(child_touch_source, child_events);

  // Scene is now set up, send in the input. Inject tap events on the four corners of the display.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref));

  Inject(0, 0, fupi::EventPhase::kAdd);
  Inject(0, display_height_, fupi::EventPhase::kChange);
  Inject(display_width_, 0, fupi::EventPhase::kChange);
  Inject(display_width_, display_height_, fupi::EventPhase::kRemove);

  // child_view should receive all the input events.
  RunLoopUntil([&child_events] { return child_events.size() == 4u; });

  ASSERT_TRUE(child_events[0].view_parameters().has_value());
  auto viewport_to_view_transform = child_events[0].view_parameters()->viewport_to_view_transform();

  // As we had scaled the child_view with a factor of 2 and translated it by
  // (display_width_/2,display_height_/2), the tap event injected at coordinates (0,0) would be
  // transformed to (-display_width_/4,-display_height_/4).
  {
    const auto& event = child_events[0];
    const auto expected_phase = EventPhase::kAdd;
    const auto expected_x = (0 - static_cast<float>(translation_x)) / scale_x;
    const auto expected_y = (0 - static_cast<float>(translation_y)) / scale_y;
    EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, expected_phase,
                      expected_x, expected_y);
  }

  // As we had scaled the child_view with a factor of 2 and translated it by
  // (display_width_/2,display_height_/2), the tap event injected at coordinates (0,display_height_)
  // would be transformed to (-display_width_/4,display_height_/4).
  {
    const auto& event = child_events[1];
    const auto expected_phase = EventPhase::kChange;
    const auto expected_x = (0 - static_cast<float>(translation_x)) / scale_x;
    const auto expected_y = (display_height_ - static_cast<float>(translation_y)) / scale_y;
    EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, expected_phase,
                      expected_x, expected_y);
  }

  // As we had scaled the child_view with a factor of 2 and translated it by
  // (display_width_/2,display_height_/2), the tap event injected at coordinates (display_width_,0)
  // would be transformed to (display_width_/4,-display_height_/4).
  {
    const auto& event = child_events[2];
    const auto expected_phase = EventPhase::kChange;
    const auto expected_x = (display_width_ - static_cast<float>(translation_x)) / scale_x;
    const auto expected_y = (0 - static_cast<float>(translation_y)) / scale_y;
    EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, expected_phase,
                      expected_x, expected_y);
  }

  // As we had scaled the child_view with a factor of 2 and translated it by
  // (display_width_/2,display_height_/2), the tap event injected at coordinates
  // (display_width_,display_height_) would be transformed to (display_width_/4,display_height_/4).
  {
    const auto& event = child_events[3];
    const auto expected_phase = EventPhase::kRemove;
    const auto expected_x = (display_width_ - static_cast<float>(translation_x)) / scale_x;
    const auto expected_y = (display_height_ - static_cast<float>(translation_y)) / scale_y;
    EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, expected_phase,
                      expected_x, expected_y);
  }
}

}  // namespace integration_tests
