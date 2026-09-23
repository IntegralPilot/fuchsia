// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.input.report/cpp/fidl.h>
#include <fidl/fuchsia.input/cpp/fidl.h>
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
// - The test fixture sets up the display + the root instance and view.
// - Injection done in context View Space, with fuchsia.ui.pointerinjector
// - Target(s) specified by View (using view ref koids)
// - Dispatch done to fuchsia.ui.pointer.MouseSource in receiver View Space.
namespace integration_tests {

namespace fir = fuchsia_input_report;
namespace fuc = fuchsia_ui_composition;
namespace fup = fuchsia_ui_pointer;
namespace fupi = fuchsia_ui_pointerinjector;
namespace fuv = fuchsia_ui_views;

using MouseEvent = fup::MouseEvent;
using TransformId = fuc::TransformId;
using ContentId = fuc::ContentId;
using HitRegion = fuc::HitRegion;
using HitTestInteraction = fuc::HitTestInteraction;
using Orientation = fuc::Orientation;
using ViewportProperties = fuc::ViewportProperties;
using EventPhase = fupi::EventPhase;
using DispatchPolicy = fupi::DispatchPolicy;
using MouseViewStatus = fup::MouseViewStatus;
using FocusState = fuv::FocusState;
using ViewRef = fuv::ViewRef;
using Rect = fuchsia_math::Rect;
using RectF = fuchsia_math::RectF;
using Vec = fuchsia_math::Vec;
using VecF = fuchsia_math::VecF;
using SizeU = fuchsia_math::SizeU;

// Macros for calling EXPECT on fuchsia_ui_pointer::MousePointerSample.
// Delegates to ExpectEqualPointer(), but are macros to ensure we get the correct line number for
// the error.
#define EXPECT_EQ_POINTER_WITH_SCROLL_AND_BUTTONS(pointer_sample, viewport_to_view_transform, \
                                                  expected_x, expected_y, expected_scroll_v,  \
                                                  expected_scroll_h, expected_buttons)        \
  ExpectEqualPointer(pointer_sample, viewport_to_view_transform, expected_x, expected_y,      \
                     expected_scroll_v, expected_scroll_h, expected_buttons, __LINE__);

#define EXPECT_EQ_POINTER_WITH_SCROLL(pointer_sample, viewport_to_view_transform, expected_x, \
                                      expected_y, expected_scroll_v, expected_scroll_h)       \
  EXPECT_EQ_POINTER_WITH_SCROLL_AND_BUTTONS(pointer_sample, viewport_to_view_transform,       \
                                            expected_x, expected_y, expected_scroll_v,        \
                                            expected_scroll_h, std::vector<uint8_t>());

#define EXPECT_EQ_POINTER_WITH_BUTTONS(pointer_sample, viewport_to_view_transform, expected_x, \
                                       expected_y, expected_buttons)                           \
  EXPECT_EQ_POINTER_WITH_SCROLL_AND_BUTTONS(pointer_sample, viewport_to_view_transform,        \
                                            expected_x, expected_y, std::optional<int64_t>(),  \
                                            std::optional<int64_t>(), expected_buttons);

#define EXPECT_EQ_POINTER(pointer_sample, viewport_to_view_transform, expected_x, expected_y) \
  EXPECT_EQ_POINTER_WITH_BUTTONS(pointer_sample, viewport_to_view_transform, expected_x,      \
                                 expected_y, std::vector<uint8_t>());

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

void ExpectEqualPointer(const std::optional<fup::MousePointerSample>& pointer_sample,
                        const std::array<float, 9>& viewport_to_view_transform, float expected_x,
                        float expected_y, std::optional<int64_t> expected_scroll_v,
                        std::optional<int64_t> expected_scroll_h,
                        std::vector<uint8_t> expected_buttons, uint32_t line_number) {
  ASSERT_TRUE(pointer_sample.has_value(), "Line: %d", line_number);
  const Mat3 transform_matrix = ArrayToMat3(viewport_to_view_transform);
  ASSERT_TRUE(pointer_sample->position_in_viewport().has_value(), "Line: %d", line_number);
  const std::array<float, 2> transformed_pointer =
      TransformPointerCoords(pointer_sample->position_in_viewport().value(), transform_matrix);
  EXPECT_TRUE(CmpFloatingValues(transformed_pointer[0], expected_x), "Line: %d", line_number);
  EXPECT_TRUE(CmpFloatingValues(transformed_pointer[1], expected_y), "Line: %d", line_number);
  if (expected_scroll_v.has_value()) {
    ASSERT_TRUE(pointer_sample->scroll_v().has_value(), "Line: %d", line_number);
    EXPECT_EQ(pointer_sample->scroll_v().value(), expected_scroll_v.value(), "Line: %d",
              line_number);
  } else {
    EXPECT_FALSE(pointer_sample->scroll_v().has_value(), "Line: %d", line_number);
  }
  if (expected_scroll_h.has_value()) {
    ASSERT_TRUE(pointer_sample->scroll_h().has_value(), "Line: %d", line_number);
    EXPECT_EQ(pointer_sample->scroll_h().value(), expected_scroll_h.value(), "Line: %d",
              line_number);
  } else {
    EXPECT_FALSE(pointer_sample->scroll_h().has_value(), "Line: %d", line_number);
  }
  if (expected_buttons.empty()) {
    EXPECT_FALSE(pointer_sample->pressed_buttons().has_value(), "Line: %d", line_number);
  } else {
    ASSERT_TRUE(pointer_sample->pressed_buttons().has_value(), "Line: %d", line_number);
    ASSERT_EQ(pointer_sample->pressed_buttons()->size(), expected_buttons.size());
    for (size_t i = 0; i < pointer_sample->pressed_buttons()->size(); i++) {
      EXPECT_EQ(pointer_sample->pressed_buttons()->at(i), expected_buttons[i], "Line: %d",
                line_number);
    }
  }
}

}  // namespace

class MouseSourceV2Client : public fidl::AsyncEventHandler<fup::MouseSourceV2> {
 public:
  MouseSourceV2Client(fidl::ClientEnd<fup::MouseSourceV2> client_end,
                      async_dispatcher_t* dispatcher, std::vector<fup::MouseEvent>& out_events)
      : client_(std::move(client_end), dispatcher, this), out_events_(out_events) {}

  void on_fidl_error(fidl::UnbindInfo info) override { is_bound_ = false; }
  void handle_unknown_event(fidl::UnknownEventMetadata<fup::MouseSourceV2> metadata) override {}

  void OnMouseEvents(fidl::Event<fup::MouseSourceV2::OnMouseEvents>& event) override {
    std::move(event.events().begin(), event.events().end(), std::back_inserter(out_events_));
    EXPECT_TRUE(
        client_->AcknowledgeEvents({{.last_acknowledged_event_stamp = event.last_event_stamp()}})
            .is_ok());
  }

  bool is_bound() const { return is_bound_ && client_.is_valid(); }
  fidl::Client<fup::MouseSourceV2>& operator->() { return client_; }

 private:
  bool is_bound_ = true;
  fidl::Client<fup::MouseSourceV2> client_;
  std::vector<fup::MouseEvent>& out_events_;
};

class FlatlandMouseIntegrationTest : public ScenicCtfTest {
 protected:
  static constexpr uint32_t kDeviceId = 1111;
  static constexpr uint32_t kPointerId = 2222;
  static constexpr uint32_t kDefaultSize = 10;

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

    // Set up root view and root transform.
    root_instance_ = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    root_instance_->set_on_close(FailOnClose("Lost connection to Scenic"));

    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    root_view_ref_ = scenic::cpp::CloneViewRef(identity.view_ref());

    ASSERT_TRUE((*root_instance_)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = {},
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());

    SetFlatlandDisplayContent(std::move(parent_token));

    ASSERT_TRUE((*root_instance_)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    ASSERT_TRUE((*root_instance_)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *root_instance_);

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

  void Inject(float x, float y, fupi::EventPhase phase,
              const std::vector<uint8_t>& pressed_buttons = {},
              std::optional<int64_t> scroll_v = std::nullopt,
              std::optional<int64_t> scroll_h = std::nullopt,
              std::optional<double> scroll_v_physical_pixel = std::nullopt,
              std::optional<double> scroll_h_physical_pixel = std::nullopt,
              std::optional<bool> is_precision_scroll = std::nullopt) {
    FX_CHECK(injector_);
    fupi::Event event;
    event.timestamp(0);
    {
      fupi::PointerSample pointer_sample;
      pointer_sample.pointer_id(kPointerId);
      pointer_sample.phase(phase);
      pointer_sample.position_in_viewport(std::array<float, 2>{x, y});
      if (scroll_v.has_value()) {
        pointer_sample.scroll_v(scroll_v.value());
      }
      if (scroll_h.has_value()) {
        pointer_sample.scroll_h(scroll_h.value());
      }
      if (scroll_v_physical_pixel.has_value()) {
        pointer_sample.scroll_v_physical_pixel(scroll_v_physical_pixel.value());
      }
      if (scroll_h_physical_pixel.has_value()) {
        pointer_sample.scroll_h_physical_pixel(scroll_h_physical_pixel.value());
      }
      if (is_precision_scroll.has_value()) {
        pointer_sample.is_precision_scroll(is_precision_scroll.value());
      }

      if (!pressed_buttons.empty()) {
        pointer_sample.pressed_buttons(pressed_buttons);
      }
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

  void RegisterInjector(fuv::ViewRef context_view_ref, fuv::ViewRef target_view_ref,
                        fupi::DispatchPolicy dispatch_policy, std::vector<uint8_t> buttons,
                        std::array<float, 9> viewport_to_context_transform) {
    fupi::Config config;
    config.device_id(kDeviceId);
    config.device_type(fupi::DeviceType::kMouse);
    config.dispatch_policy(dispatch_policy);

    {
      fir::Axis axis(fuchsia_input::Range(-1, 1),
                     fuchsia_input::Unit(fuchsia_input::UnitType::kNone, 0));
      config.scroll_v_range(axis);
      config.scroll_h_range(axis);
    }

    config.buttons(std::move(buttons));
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

  // Starts a recursive MouseSource::Watch() loop that collects all received events into
  // |out_events|.
  void StartWatchLoop(SimpleWatcherClient<fup::MouseSource>& mouse_source,
                      std::vector<fup::MouseEvent>& out_events) {
    const size_t index = watch_loops_.size();
    watch_loops_.emplace_back();
    watch_loops_.at(index) = [this, &mouse_source, &out_events,
                              index](std::vector<fup::MouseEvent> events) {
      std::move(events.begin(), events.end(), std::back_inserter(out_events));
      mouse_source->Watch().Then([this, index](fidl::Result<fup::MouseSource::Watch>& result) {
        if (result.is_ok()) {
          watch_loops_.at(index)(std::move(result->events()));
        }
      });
    };
    mouse_source->Watch().Then([this, index](fidl::Result<fup::MouseSource::Watch>& result) {
      if (result.is_ok()) {
        watch_loops_.at(index)(std::move(result->events()));
      }
    });
  }

  // Convenience function, we assume the test constructs topologies with one level of N children.
  // Prereq: |parent_of_viewport_transform| is created and connected to the view's root.
  fuv::ViewRef CreateAndAddChildView(
      FlatlandClientWithEventHandler& parent_instance, fuc::TransformId viewport_transform_id,
      fuc::TransformId parent_of_viewport_transform, fuc::ContentId parent_content_id,
      std::unique_ptr<FlatlandClientWithEventHandler>& child_instance,
      fidl::ServerEnd<fup::MouseSource> child_mouse_source = {},
      fidl::ServerEnd<fuv::ViewRefFocused> child_focused = {}) {
    child_instance = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    child_instance->set_on_close(FailOnClose("Lost connection to Scenic"));

    // Set up the child view watcher.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    fuc::ViewportProperties properties;
    properties.logical_size(fuchsia_math::SizeU(kDefaultSize, kDefaultSize));

    EXPECT_TRUE(
        parent_instance->CreateTransform({{.transform_id = viewport_transform_id}}).is_ok());
    EXPECT_TRUE(
        parent_instance
            ->CreateViewport({{.viewport_id = parent_content_id,
                               .token = std::move(parent_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE(
        parent_instance
            ->SetContent({{.transform_id = viewport_transform_id, .content_id = parent_content_id}})
            .is_ok());
    EXPECT_TRUE(parent_instance
                    ->AddChild({{.parent_transform_id = parent_of_viewport_transform,
                                 .child_transform_id = viewport_transform_id}})
                    .is_ok());

    BlockingPresent(this, parent_instance);

    // Set up the child view along with its MouseSource and ViewRefFocused channel.
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    fuc::ViewBoundProtocols protocols;
    if (child_mouse_source.is_valid())
      protocols.mouse_source(std::move(child_mouse_source));
    if (child_focused.is_valid())
      protocols.view_ref_focused(std::move(child_focused));
    EXPECT_TRUE((*child_instance)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_instance)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_instance)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *child_instance);

    return child_view_ref;
  }

  fuv::ViewRef CreateAndAddChildViewV2(
      FlatlandClientWithEventHandler& parent_instance, fuc::TransformId viewport_transform_id,
      fuc::TransformId parent_of_viewport_transform, fuc::ContentId parent_content_id,
      std::unique_ptr<FlatlandClientWithEventHandler>& child_instance,
      fidl::ServerEnd<fup::MouseSourceV2> child_mouse_source_v2 = {},
      fidl::ServerEnd<fuv::ViewRefFocused> child_focused = {}) {
    child_instance = std::make_unique<FlatlandClientWithEventHandler>(
        ConnectIntoRealm<fuc::Flatland>(), dispatcher());
    child_instance->set_on_close(FailOnClose("Lost connection to Scenic"));

    // Set up the child view watcher.
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    fuc::ViewportProperties properties;
    properties.logical_size(fuchsia_math::SizeU(kDefaultSize, kDefaultSize));

    EXPECT_TRUE(
        parent_instance->CreateTransform({{.transform_id = viewport_transform_id}}).is_ok());
    EXPECT_TRUE(
        parent_instance
            ->CreateViewport({{.viewport_id = parent_content_id,
                               .token = std::move(parent_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE(
        parent_instance
            ->SetContent({{.transform_id = viewport_transform_id, .content_id = parent_content_id}})
            .is_ok());
    EXPECT_TRUE(parent_instance
                    ->AddChild({{.parent_transform_id = parent_of_viewport_transform,
                                 .child_transform_id = viewport_transform_id}})
                    .is_ok());

    BlockingPresent(this, parent_instance);

    // Set up the child view along with its MouseSourceV2 and ViewRefFocused channel.
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    auto identity = scenic::cpp::NewViewIdentityOnCreation();
    auto child_view_ref = scenic::cpp::CloneViewRef(identity.view_ref());
    fuc::ViewBoundProtocols protocols;
    if (child_mouse_source_v2.is_valid())
      protocols.mouse_source_v2(std::move(child_mouse_source_v2));
    if (child_focused.is_valid())
      protocols.view_ref_focused(std::move(child_focused));
    EXPECT_TRUE((*child_instance)
                    ->CreateView2({{.token = std::move(child_token),
                                    .view_identity = std::move(identity),
                                    .protocols = std::move(protocols),
                                    .parent_viewport_watcher =
                                        std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_instance)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_instance)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *child_instance);

    return child_view_ref;
  }

  std::array<std::array<float, 2>, 2> FullScreenExtents() const {
    return {{{0, 0}, {display_width_, display_height_}}};
  }

  std::unique_ptr<FlatlandClientWithEventHandler> root_instance_;
  fuv::ViewRef root_view_ref_;
  float display_width_ = 0;
  float display_height_ = 0;

  std::unique_ptr<SimpleWatcherClient<fupi::Device>> injector_;

 private:
  fidl::SyncClient<fupi::Registry> pointerinjector_registry_;

  // Holds watch loops so they stay alive through the duration of the test.
  std::vector<std::function<void(std::vector<fup::MouseEvent>)>> watch_loops_;
};

TEST_F(FlatlandMouseIntegrationTest, ReleaseTargetView_TriggersChannelClosure) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  // Break the scene graph relation that the pointerinjector relies on. Observe the channel close
  // (lazily).
  EXPECT_TRUE((*child_instance)->ReleaseView().is_ok());
  BlockingPresent(this, *child_instance);

  // Inject an event to trigger the channel closure.
  Inject(0, 0, EventPhase::kAdd, button_vec);
  RunLoopUntil([this] { return !injector_->is_bound(); });  // Succeeds or times out.
}

TEST_F(FlatlandMouseIntegrationTest, DisconnectTargetView_TriggersChannelClosure) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  // Break the scene graph relation that the pointerinjector relies on. Observe the channel close
  // (lazily).
  EXPECT_TRUE((*root_instance_)
                  ->RemoveChild({{.parent_transform_id = kRootTransform,
                                  .child_transform_id = kViewportTransform}})
                  .is_ok());
  BlockingPresent(this, *root_instance_);

  // Inject an event to trigger the channel closure.
  Inject(0, 0, EventPhase::kAdd, button_vec);
  RunLoopUntil([this] { return !injector_->is_bound(); });  // Succeeds or times out.
}

// The child view should receive focus and input events when the mouse button is pressed over its
// view.
TEST_F(FlatlandMouseIntegrationTest, ChildReceivesFocus_OnMouseLatch) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));
  auto [child_focused_client_end, child_focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_focused(
      std::move(child_focused_client_end), dispatcher(), FailOnClose("ViewRefFocused closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end),
                                              std::move(child_focused_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);

  // Child should receive mouse input events.
  RunLoopUntil([&child_events] { return child_events.size() == 1u; });

  // Child view should receive focus.
  std::optional<FocusState> is_child_focused;
  child_focused->Watch().Then(
      [&is_child_focused](fidl::Result<fuv::ViewRefFocused::Watch>& result) {
        if (result.is_ok()) {
          is_child_focused = std::move(result->state());
        }
      });
  RunLoopUntil([&is_child_focused] { return is_child_focused.has_value(); });
  EXPECT_TRUE(is_child_focused->focused().value());
}

TEST_F(FlatlandMouseIntegrationTest, MouseRejectsFocus_OnMouseLatchWithInvalidContext) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));
  auto [child_focused_client_end, child_focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_focused(
      std::move(child_focused_client_end), dispatcher(), FailOnClose("ViewRefFocused closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end),
                                              std::move(child_focused_server_end));

  // Create a sibling view to act as the invalid injection context.
  std::unique_ptr<FlatlandClientWithEventHandler> sibling_instance;
  auto [sibling_mouse_source_client_end, sibling_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> sibling_mouse_source(
      std::move(sibling_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));
  auto [sibling_focused_client_end, sibling_focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> sibling_focused(
      std::move(sibling_focused_client_end), dispatcher(), FailOnClose("ViewRefFocused closed"));
  auto sibling_view_ref = CreateAndAddChildView(
      *root_instance_,
      /*viewport_transform_id*/ TransformId(3),
      /*parent_of_viewport_transform*/ kRootTransform,
      /*parent_content_id*/ ContentId(2), sibling_instance,
      std::move(sibling_mouse_source_server_end), std::move(sibling_focused_server_end));

  // Create a child of the sibling view to act as the injection target.
  std::unique_ptr<FlatlandClientWithEventHandler> child_of_sibling_instance;
  auto [child_of_sibling_mouse_source_client_end, child_of_sibling_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_of_sibling_mouse_source(
      std::move(child_of_sibling_mouse_source_client_end), dispatcher(),
      FailOnClose("Mouse source closed"));
  auto [child_of_sibling_focused_client_end, child_of_sibling_focused_server_end] =
      fidl::CreateEndpoints<fuv::ViewRefFocused>().value();
  SimpleWatcherClient<fuv::ViewRefFocused> child_of_sibling_focused(
      std::move(child_of_sibling_focused_client_end), dispatcher(),
      FailOnClose("ViewRefFocused closed"));
  auto child_of_sibling_view_ref =
      CreateAndAddChildView(*sibling_instance,
                            /*viewport_transform_id*/ kViewportTransform,
                            /*parent_of_viewport_transform*/ kRootTransform,
                            /*parent_content_id*/ ContentId(1), child_of_sibling_instance,
                            std::move(child_of_sibling_mouse_source_server_end),
                            std::move(child_of_sibling_focused_server_end));

  // Listen for input events on the injection target.
  std::vector<MouseEvent> child_of_sibling_events;
  StartWatchLoop(child_of_sibling_mouse_source, child_of_sibling_events);

  // Setup an injector where sibling is context, and child of sibling is target.
  // This passes the pointerinjector registry validation (target is strict descendant of context).
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(sibling_view_ref),
                   scenic::cpp::CloneViewRef(child_of_sibling_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);

  // Target should receive the mouse input events.
  RunLoopUntil([&child_of_sibling_events] { return child_of_sibling_events.size() == 1u; });

  // Sibling view should NOT receive focus because `sibling` is the context (and thus the
  // requester), but `sibling` is not currently in the focus chain. It has no authority to request
  // focus.
  std::optional<FocusState> is_sibling_focused;
  sibling_focused->Watch().Then(
      [&is_sibling_focused](fidl::Result<fuv::ViewRefFocused::Watch>& result) {
        if (result.is_ok()) {
          is_sibling_focused = std::move(result->state());
        }
      });
  RunLoopWithTimeout(zx::msec(50));
  EXPECT_FALSE(is_sibling_focused.has_value());
}

// Send wheel events to scenic ensure client receives wheel events.
TEST_F(FlatlandMouseIntegrationTest, Wheel) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1), /* scroll_h= */ std::optional<int64_t>(-1));

  RunLoopUntil([&child_events] { return child_events.size() == 2u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_v().value(), 1);
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h().value(), -1);
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->is_precision_scroll().has_value());
}

// Send wheel events in button pressing sequence to scenic ensure client receives correct wheel
// events.
TEST_F(FlatlandMouseIntegrationTest, DownWheelUpWheel) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);
  Inject(0, 0, EventPhase::kChange, button_vec);
  Inject(0, 0, EventPhase::kChange, button_vec,
         /* scroll_v= */ std::optional<int64_t>(1));
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {});
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1));

  RunLoopUntil([&child_events] { return child_events.size() == 5u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->pressed_buttons().value(), button_vec);

  ASSERT_TRUE(child_events[2].pointer_sample().has_value());
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v().value(), 1);
  EXPECT_FALSE(child_events[2].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->pressed_buttons().value(), button_vec);
  EXPECT_FALSE(child_events[2].pointer_sample()->is_precision_scroll().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[3].pointer_sample().has_value());
  EXPECT_FALSE(child_events[3].pointer_sample()->pressed_buttons().has_value());
  EXPECT_FALSE(child_events[3].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[3].pointer_sample()->is_precision_scroll().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[4].pointer_sample().has_value());
  ASSERT_TRUE(child_events[4].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[4].pointer_sample()->scroll_v().value(), 1);
  EXPECT_FALSE(child_events[4].pointer_sample()->scroll_h().has_value());
  EXPECT_FALSE(child_events[4].pointer_sample()->pressed_buttons().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->is_precision_scroll().has_value());
}

// Send wheel events bundled with button changess to scenic ensure client receives correct wheel
// events.
TEST_F(FlatlandMouseIntegrationTest, DownWheelUpWheelBundled) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);
  // This event bundled button down and wheel.
  Inject(0, 0, EventPhase::kChange, button_vec, /* scroll_v= */ std::optional<int64_t>(1));
  Inject(0, 0, EventPhase::kChange, button_vec, /* scroll_v= */ std::optional<int64_t>(1));
  // This event bundled button up and wheel.
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1));
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1));

  RunLoopUntil([&child_events] { return child_events.size() == 5u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_v().value(), 1);
  EXPECT_EQ(child_events[1].pointer_sample()->pressed_buttons().value(), button_vec);
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[2].pointer_sample().has_value());
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v().value(), 1);
  EXPECT_EQ(child_events[2].pointer_sample()->pressed_buttons().value(), button_vec);
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[3].pointer_sample().has_value());
  ASSERT_TRUE(child_events[3].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[3].pointer_sample()->scroll_v().value(), 1);
  EXPECT_FALSE(child_events[3].pointer_sample()->pressed_buttons().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[3].pointer_sample()->is_precision_scroll().has_value());

  ASSERT_TRUE(child_events[4].pointer_sample().has_value());
  ASSERT_TRUE(child_events[4].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[4].pointer_sample()->scroll_v().value(), 1);
  EXPECT_FALSE(child_events[4].pointer_sample()->pressed_buttons().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[4].pointer_sample()->is_precision_scroll().has_value());
}

// Send wheel events with physical pixel fields to scenic ensure client receives wheel events.
TEST_F(FlatlandMouseIntegrationTest, WheelWithPhysicalPixel) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);

  RunLoopUntil([&child_events] { return child_events.size() == 1u; });
  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->is_precision_scroll().has_value());
  child_events.clear();

  // with v physical pixel, not precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1),
         /* scroll_h= */ std::nullopt,
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::nullopt,
         /* is_precision_scroll= */ std::optional<bool>(false));

  // with h physical pixel, not precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::nullopt,
         /* scroll_h= */ std::optional<int64_t>(-1),
         /* scroll_v_physical_pixel= */ std::nullopt,
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(false));

  // with v,h physical pixel, not precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1),
         /* scroll_h= */ std::optional<int64_t>(-1),
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(false));

  RunLoopUntil([&child_events] { return child_events.size() == 3u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[0].pointer_sample()->scroll_v().value(), 1);
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[0].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h().value(), -1);
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[1].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_FALSE(child_events[1].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[2].pointer_sample().has_value());
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v().value(), 1);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_h().value(), -1);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_FALSE(child_events[2].pointer_sample()->is_precision_scroll().value());

  child_events.clear();

  // with v physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1),
         /* scroll_h= */ std::nullopt,
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::nullopt,
         /* is_precision_scroll= */ std::optional<bool>(true));

  // with h physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::nullopt,
         /* scroll_h= */ std::optional<int64_t>(-1),
         /* scroll_v_physical_pixel= */ std::nullopt,
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(true));

  // with v,h physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::optional<int64_t>(1),
         /* scroll_h= */ std::optional<int64_t>(-1),
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(true));

  RunLoopUntil([&child_events] { return child_events.size() == 3u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[0].pointer_sample()->scroll_v().value(), 1);
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[0].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[0].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h().value(), -1);
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[1].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[1].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[2].pointer_sample().has_value());
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v().value(), 1);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_h().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_h().value(), -1);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[2].pointer_sample()->is_precision_scroll().value());

  child_events.clear();

  // without tick, with v physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::nullopt,
         /* scroll_h= */ std::nullopt,
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::nullopt,

         /* is_precision_scroll= */ std::optional<bool>(true));

  // without tick, with h physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::nullopt,
         /* scroll_h= */ std::nullopt,
         /* scroll_v_physical_pixel= */ std::nullopt,
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(true));

  // without tick, with v,h physical pixel, is precision scroll
  Inject(0, 0, EventPhase::kChange, /* pressed_buttons= */ {},
         /* scroll_v= */ std::nullopt,
         /* scroll_h= */ std::nullopt,
         /* scroll_v_physical_pixel= */ std::optional<double>(120.0),
         /* scroll_h_physical_pixel= */ std::optional<double>(-120.0),
         /* is_precision_scroll= */ std::optional<bool>(true));

  RunLoopUntil([&child_events] { return child_events.size() == 3u; });

  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[0].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_FALSE(child_events[0].pointer_sample()->scroll_h_physical_pixel().has_value());
  ASSERT_TRUE(child_events[0].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[0].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_h().has_value());
  ASSERT_FALSE(child_events[1].pointer_sample()->scroll_v_physical_pixel().has_value());
  ASSERT_TRUE(child_events[1].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[1].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[1].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[1].pointer_sample()->is_precision_scroll().value());

  ASSERT_TRUE(child_events[2].pointer_sample().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_v().has_value());
  ASSERT_FALSE(child_events[2].pointer_sample()->scroll_h().has_value());
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_v_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_v_physical_pixel().value(), 120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->scroll_h_physical_pixel().has_value());
  EXPECT_EQ(child_events[2].pointer_sample()->scroll_h_physical_pixel().value(), -120.0);
  ASSERT_TRUE(child_events[2].pointer_sample()->is_precision_scroll().has_value());
  EXPECT_TRUE(child_events[2].pointer_sample()->is_precision_scroll().value());
}

// Hit tests follow the same basic view topology:
//
// root_instance     - context view
//     |
//     |
// parent_instance   - target view
//     |
//     |
// child_instance
//
// Only the parent and child instances are eligible to receive hits. This is based on whether they
// have a hit region for a given (x,y), and on the local transform topology of |parent_instance|.
// Simply put, the precedence for hits goes towards the transforms added *last* in the
// parent_instance's local topology.

// Add full screen hit regions on both parent and child instances. Check that only the child
// receives hits.
TEST_F(FlatlandMouseIntegrationTest, SimpleHitTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_instance;
  auto [parent_mouse_source_client_end, parent_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> parent_mouse_source(
      std::move(parent_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto parent_view_ref = CreateAndAddChildView(*root_instance_,
                                               /*viewport_transform_id*/ kViewportTransform,
                                               /*parent_of_viewport_transform*/ kRootTransform,
                                               /*parent_content_id*/ ContentId(1), parent_instance,
                                               std::move(parent_mouse_source_server_end));

  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(
      *parent_instance, /*viewport_transform_id=*/kViewportTransform, kRootTransform,
      /*parent_content_id=*/ContentId(2), child_instance, std::move(child_mouse_source_server_end));

  // Place hit regions, overriding any default ones if they exist.
  EXPECT_TRUE((*parent_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());
  EXPECT_TRUE((*child_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());

  BlockingPresent(this, *child_instance);
  BlockingPresent(this, *parent_instance);

  // Listen for input events.
  std::vector<MouseEvent> parent_events;
  StartWatchLoop(parent_mouse_source, parent_events);

  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is the point of overlap between the parent and the
  // child. The child should receive it.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(parent_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);

  RunLoopUntil([&child_events] { return child_events.size() == 1u; });
  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());

  // Verify hit position in viewport.
  std::array<float, 2> position = child_events[0].pointer_sample()->position_in_viewport().value();

  EXPECT_EQ(position[0], 0.f);
  EXPECT_EQ(position[1], 0.f);

  // Parent should have received 0 events.
  EXPECT_EQ(parent_events.size(), 0u);
}

// Add full screen hit regions for both parent and child instances. This time, the parent adds an
// additional partial-screen overlay on top of the child, which should receive hits instead of the
// child for that portion of the screen. This forms a parent-child-parent "sandwich" for that
// region.
TEST_F(FlatlandMouseIntegrationTest, SandwichTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_instance;
  auto [parent_mouse_source_client_end, parent_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> parent_mouse_source(
      std::move(parent_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto parent_view_ref = CreateAndAddChildView(*root_instance_,
                                               /*viewport_transform_id*/ kViewportTransform,
                                               /*parent_of_viewport_transform*/ kRootTransform,
                                               /*parent_content_id*/ ContentId(1), parent_instance,
                                               std::move(parent_mouse_source_server_end));

  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(
      *parent_instance, /*parent_transform=*/kViewportTransform, kRootTransform,
      /*parent_content_id=*/ContentId(2), child_instance, std::move(child_mouse_source_server_end));

  // After creating the child transform, create an additional transform representing the overlay.
  TransformId overlay_transform(3);
  EXPECT_TRUE((*parent_instance)->CreateTransform({{.transform_id = overlay_transform}}).is_ok());
  EXPECT_TRUE((*parent_instance)
                  ->AddChild({{.parent_transform_id = kRootTransform,
                               .child_transform_id = overlay_transform}})
                  .is_ok());

  // Place hit regions, overriding any default ones if they exist.
  EXPECT_TRUE((*parent_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());
  EXPECT_TRUE((*parent_instance)
                  ->SetHitRegions(
                      {{.transform_id = overlay_transform,
                        .regions = {HitRegion(RectF(0, 0, 5, 5), HitTestInteraction::kDefault)}}})
                  .is_ok());
  EXPECT_TRUE((*child_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());

  BlockingPresent(this, *child_instance);
  BlockingPresent(this, *parent_instance);

  // Listen for input events.
  std::vector<MouseEvent> parent_events;
  StartWatchLoop(parent_mouse_source, parent_events);

  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Inject an input event at (0,0) which is in the sandwich zone. The parent should receive it.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(parent_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd, button_vec);
  RunLoopUntil([&parent_events] { return parent_events.size() == 1u; });
  ASSERT_TRUE(parent_events[0].pointer_sample().has_value());
  EXPECT_FALSE(parent_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(parent_events[0].pointer_sample()->scroll_h().has_value());

  // Verify hit position in viewport.
  {
    std::array<float, 2> position =
        parent_events[0].pointer_sample()->position_in_viewport().value();

    EXPECT_EQ(position[0], 0.f);
    EXPECT_EQ(position[1], 0.f);
  }

  // Remove the previous stream.
  Inject(0, 0, EventPhase::kRemove, {});
  RunLoopUntil([&parent_events] { return parent_events.size() == 2u; });
  EXPECT_EQ(child_events.size(), 0u);

  // Inject outside of the sandwich zone. The child should receive it.
  Inject(6, 3, EventPhase::kAdd, button_vec);

  RunLoopUntil([&child_events] { return child_events.size() == 1u; });
  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_v().has_value());
  EXPECT_FALSE(child_events[0].pointer_sample()->scroll_h().has_value());

  // Verify hit position in viewport.
  {
    std::array<float, 2> position =
        child_events[0].pointer_sample()->position_in_viewport().value();

    EXPECT_EQ(position[0], 6.f);
    EXPECT_EQ(position[1], 3.f);
  }

  // Parent should have received 0 additional events.
  EXPECT_EQ(parent_events.size(), 2u);
}

// In order to test that partial screen views work - this test establishes a context view that is
// translated away from the root view.
//
// ------------------
// |(Root)          |
// |                |
// |                |
// |                |
// |        --------|
// |        |(C/T)  |
// |        |       |
// |        |       |
// ------------------
//
// Root view: 10x10 with origin at (0,0)
// Context and target views: 5x5 with origin at (5,5)
//
//
// root parent context target
TEST_F(FlatlandMouseIntegrationTest, PartialScreenViews) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_instance;
  auto [parent_mouse_source_client_end, parent_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> parent_mouse_source(
      std::move(parent_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto parent_view_ref = CreateAndAddChildView(*root_instance_,
                                               /*viewport_transform_id*/ kViewportTransform,
                                               /*parent_of_viewport_transform*/ kRootTransform,
                                               /*parent_content_id*/ ContentId(1), parent_instance,
                                               std::move(parent_mouse_source_server_end));

  std::unique_ptr<FlatlandClientWithEventHandler> context_instance;
  auto [context_mouse_source_client_end, context_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> context_mouse_source(
      std::move(context_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto context_view_ref =
      CreateAndAddChildView(*parent_instance, kViewportTransform, kRootTransform,
                            /*parent_content_id=*/ContentId(2), context_instance,
                            std::move(context_mouse_source_server_end));

  std::unique_ptr<FlatlandClientWithEventHandler> target_instance;
  auto [target_mouse_source_client_end, target_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> target_mouse_source(
      std::move(target_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto target_view_ref =
      CreateAndAddChildView(*context_instance, kViewportTransform, kRootTransform,
                            /*parent_content_id=*/ContentId(2), target_instance,
                            std::move(target_mouse_source_server_end));

  // Change the context view's origin from (0,0) to (5,5).
  int x_translation = 5;
  int y_translation = 5;
  EXPECT_TRUE((*parent_instance)
                  ->SetTranslation({{.transform_id = kViewportTransform,
                                     .translation = Vec(x_translation, y_translation)}})
                  .is_ok());
  Rect rect(0, 0, 5, 5);
  EXPECT_TRUE((*parent_instance)
                  ->SetClipBoundary(
                      {{.transform_id = kViewportTransform, .rect = std::make_unique<Rect>(rect)}})
                  .is_ok());

  // Place hit regions, overriding any default ones if they exist.
  EXPECT_TRUE((*parent_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());
  EXPECT_TRUE((*context_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());
  EXPECT_TRUE((*target_instance)
                  ->SetHitRegions(
                      {{.transform_id = kRootTransform,
                        .regions = {HitRegion(RectF(0, 0, 10, 10), HitTestInteraction::kDefault)}}})
                  .is_ok());

  BlockingPresent(this, *parent_instance);
  BlockingPresent(this, *context_instance);
  BlockingPresent(this, *target_instance);

  // Listen for input events.
  std::vector<MouseEvent> context_events;
  StartWatchLoop(context_mouse_source, context_events);

  std::vector<MouseEvent> target_events;
  StartWatchLoop(target_mouse_source, target_events);

  const std::vector<uint8_t> button_vec = {1};

  // Creates this matrix which depicts a 5x5 translation from the input viewport to the context
  // view:
  // 1 0 -5
  // 0 1 -5
  // 0 0 1
  std::array<float, 9> viewport_to_context_transform = {1, 0, 0, 0, 1, 0, -5, -5, 1};

  RegisterInjector(
      scenic::cpp::CloneViewRef(context_view_ref), scenic::cpp::CloneViewRef(target_view_ref),
      DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, viewport_to_context_transform);

  float x = 7;
  float y = 9;

  Inject(x, y, EventPhase::kAdd, button_vec);
  RunLoopUntil([&target_events] { return target_events.size() == 1u; });
  ASSERT_TRUE(target_events[0].pointer_sample().has_value());

  // Verify hit position in viewport.
  {
    std::array<float, 2> position =
        target_events[0].pointer_sample()->position_in_viewport().value();

    EXPECT_EQ(position[0], x);
    EXPECT_EQ(position[1], y);
  }

  // Parent should have received 0 events.
  EXPECT_EQ(context_events.size(), 0u);
}

// In this test we set up the context and the target. We apply a scale, rotation and translation
// transform to both of their view holder nodes, and then inject pointer events to confirm that
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
TEST_F(FlatlandMouseIntegrationTest, TargetViewWith_ScaleRotationTranslation) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Scale, rotate, and translate the child_instance. Those operations are applied in that order.
  EXPECT_TRUE((*root_instance_)
                  ->SetScale({{.transform_id = kViewportTransform, .scale = VecF(2, 3)}})
                  .is_ok());
  EXPECT_TRUE((*root_instance_)
                  ->SetOrientation({{.transform_id = kViewportTransform,
                                     .orientation = Orientation::kCcw270Degrees}})
                  .is_ok());
  EXPECT_TRUE((*root_instance_)
                  ->SetTranslation({{.transform_id = kViewportTransform, .translation = Vec(1, 0)}})
                  .is_ok());
  BlockingPresent(this, *root_instance_);

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Scene is now set up, send in the input. One event for each corner of the view.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  Inject(0, 0, EventPhase::kAdd, button_vec);
  Inject(10, 0, EventPhase::kChange, button_vec);
  Inject(0, 10, EventPhase::kChange, button_vec);
  Inject(10, 10, EventPhase::kChange, button_vec);

  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  {  // Check layout validity.
    EXPECT_EQ(child_events[0].device_info()->id(), kDeviceId);
    const auto& view_parameters = child_events[0].view_parameters().value();
    EXPECT_TRUE(CmpFloatingValues(view_parameters.view().min()[0], 0.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.view().min()[1], 0.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.view().max()[0], 10.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.view().max()[1], 10.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.viewport().min()[0], 0.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.viewport().min()[1], 0.f));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.viewport().max()[0], display_width_));
    EXPECT_TRUE(CmpFloatingValues(view_parameters.viewport().max()[1], display_height_));
  }

  // For a CCW_270 rotation, the new x' and y' from x and y is:
  // x' = y
  // y' = -x
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[0].pointer_sample(), viewport_to_view_transform,
                                   0.f / 2.f, (0.f + 1.f) / 3.f, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[1].pointer_sample(), viewport_to_view_transform,
                                   0.f / 2.f, (-10.f + 1.f) / 3.f, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[2].pointer_sample(), viewport_to_view_transform,
                                   10.f / 2.f, (0.f + 1.f) / 3.f, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[3].pointer_sample(), viewport_to_view_transform,
                                   10.f / 2.f, (-10.f + 1.f) / 3.f, button_vec);
  }
}

// In this test the context and the target have identical coordinate systems, but the viewport
// no longer matches the context's coordinate system.
//
// Below is an ASCII diagram showing the resulting setup.
// O represents the views, - the viewport.
//   X ->
// Y O   O   O   O   O   O
// |
// V   A - - - - C1- - - -
//   O - O - O - O - O - O
//     - - - - - - - - - -
//     - - - - - - - - - -
//   O - O - O - O - O - O
//     R - - - - C2- - - -
//     - - - - - - - - - -
//   O - O - O - O - O - O
//     - - - - - - - - - -
//     - - - - - - - - - -
//   O   O   O   O   O   O
//
//
//   O   O   O   O   O   O
//
TEST_F(FlatlandMouseIntegrationTest, InjectedInput_ShouldBeCorrectlyViewportTransformed) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Transform to scale the viewport by 1/2 in the x-direction, 1/3 in the y-direction,
  // and then translate by (1, 2).
  // clang-format off
  static constexpr std::array<float, 9> kViewportToContextTransform = {
    1.f/2.f,        0,  0, // first column
          0,  1.f/3.f,  0, // second column
          1,        2,  1, // third column
  };
  // clang-format on

  // Scene is now set up, send in the input. One event for each corner of the view.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(
      scenic::cpp::CloneViewRef(root_view_ref_), scenic::cpp::CloneViewRef(child_view_ref),
      DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kViewportToContextTransform);

  // Scene is now set up, send in the input. One event for where each corner of the view was
  // pre-transformation.

  Inject(0, 0, EventPhase::kAdd);                                       // A
  Inject(5, 0, EventPhase::kChange);                                    // C1
  Inject(5, 5, EventPhase::kChange);                                    // C2
  Inject(0, 5, EventPhase::kChange);                                    // R
  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  // Check pointer samples.
  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform, (0.f / 2.f) + 1,
                      (0.f / 3.f) + 2);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform, (5.f / 2.f) + 1,
                      (0.f / 3.f) + 2);
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform, (5.f / 2.f) + 1,
                      (5.f / 3.f) + 2);
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform, (0.f / 2.f) + 1,
                      (5.f / 3.f) + 2);
  }
}

// In this test the context and the target have identical coordinate systems except for a 90 degree
// rotation. Check that all corners still generate hits. This confirms that small floating point
// errors don't cause misses.
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
// Post-rotation
//   X ->
// Y 4 O O O O 1
// | O O O O O O
// v O O O O O O
//   O O O O O O
//   O O O O O O
//   3 O O O O 2
TEST_F(FlatlandMouseIntegrationTest, InjectedInput_OnRotatedChild_ShouldHitEdges) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Apply rotation.
  EXPECT_TRUE((*root_instance_)
                  ->SetOrientation({{.transform_id = kViewportTransform,
                                     .orientation = Orientation::kCcw270Degrees}})
                  .is_ok());
  EXPECT_TRUE((*root_instance_)
                  ->SetTranslation({{.transform_id = kViewportTransform, .translation = Vec(5, 0)}})
                  .is_ok());
  BlockingPresent(this, *root_instance_);

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Scene is now set up, send in the input. One interaction for each corner.
  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  Inject(0, 0, EventPhase::kAdd);
  Inject(0, 5, EventPhase::kChange);
  Inject(5, 5, EventPhase::kChange);
  Inject(5, 0, EventPhase::kChange);
  RunLoopUntil([&child_events] { return child_events.size() == 4u; });  // Succeeds or times out.

  {  // Target should receive all events rotated 90 degrees.
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER(child_events[0].pointer_sample(), viewport_to_view_transform, 0.f, 5.f);
    ASSERT_TRUE(child_events[0].stream_info().has_value());
    EXPECT_EQ(child_events[0].stream_info()->status(), MouseViewStatus::kEntered);
    EXPECT_EQ_POINTER(child_events[1].pointer_sample(), viewport_to_view_transform, 5.f, 5.f);
    EXPECT_FALSE(child_events[1].stream_info().has_value());
    EXPECT_EQ_POINTER(child_events[2].pointer_sample(), viewport_to_view_transform, 5.f, 0.f);
    EXPECT_FALSE(child_events[2].stream_info().has_value());
    EXPECT_EQ_POINTER(child_events[3].pointer_sample(), viewport_to_view_transform, 0.f, 0.f);
    EXPECT_FALSE(child_events[3].stream_info().has_value());
  }
}

// Basic scene (no transformations) where the Viewport is smaller than the Views.
// We then inject two streams: The first has an ADD outside the Viewport, which counts as a miss and
// should not be seen by anyone. The second stream has the ADD inside the Viewport and subsequent
// events outside, and this full stream should be seen by the target.
TEST_F(FlatlandMouseIntegrationTest, InjectionOutsideViewport_ShouldLimitOnClick) {
  // Set up a scene with two ViewHolders, one a child of the other. Make the Views bigger than the
  // Viewport.
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Scene is now set up, send in the input. The initial click is outside the viewport and
  // the stream should therefore not be seen by anyone.
  const uint8_t kButtonId = 1;
  const std::vector<uint8_t> button_vec = {kButtonId};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  // Set the viewport to only be the top-left quadrant of the screen.
  fupi::Viewport viewport;
  viewport.extents(std::array<std::array<float, 2>, 2>{
      {{0.f, 0.f}, {display_width_ / 2.f, display_height_ / 2.f}}});
  viewport.viewport_to_context_transform(kIdentityMatrix);
  InjectNewViewport(std::move(viewport));

  Inject(display_width_, display_height_, EventPhase::kAdd,
         button_vec);  // Outside viewport. Button down.
  // Remainder inside viewport, but should not be delivered.
  Inject(5, 0, EventPhase::kChange, button_vec);
  Inject(5, 5, EventPhase::kChange, button_vec);
  Inject(0, 5, EventPhase::kChange);  // Button up. Hover event should be delivered.

  // Send in button down starting in the viewport and moving outside.
  Inject(1, 1, EventPhase::kChange, button_vec);  // Inside viewport.
  // Remainder outside viewport, but should still be delivered.
  Inject(display_width_, 0, EventPhase::kChange, button_vec);
  Inject(display_width_, display_height_, EventPhase::kChange, button_vec);
  Inject(0, display_height_, EventPhase::kChange, button_vec);
  Inject(1, 1, EventPhase::kChange);  // Inside viewport. Button up.
  RunLoopUntil([&child_events] { return child_events.size() == 6u; });  // Succeeds or times out.

  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[0].pointer_sample(), viewport_to_view_transform,
                                   0.f, 5.f, std::vector<uint8_t>());
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[1].pointer_sample(), viewport_to_view_transform,
                                   1.f, 1.f, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[2].pointer_sample(), viewport_to_view_transform,
                                   display_width_, 0.f, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[3].pointer_sample(), viewport_to_view_transform,
                                   display_width_, display_height_, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[4].pointer_sample(), viewport_to_view_transform,
                                   0.f, display_height_, button_vec);
    EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[5].pointer_sample(), viewport_to_view_transform,
                                   1.f, 1.f, std::vector<uint8_t>());
  }
}

TEST_F(FlatlandMouseIntegrationTest, HoverTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  // Scene is now set up, send in the input. The initial click is outside the viewport and
  // the stream should therefore not be seen by anyone.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, {}, kIdentityMatrix);

  // Set the viewport to only be the top-left 9x9 section of the screen.
  fupi::Viewport viewport;
  viewport.extents(std::array<std::array<float, 2>, 2>{{{0.f, 0.f}, {9.f, 9.f}}});
  viewport.viewport_to_context_transform(kIdentityMatrix);
  InjectNewViewport(std::move(viewport));
  // Outside viewport.
  Inject(10, 10, EventPhase::kAdd);
  // Inside viewport.
  Inject(5, 0, EventPhase::kChange);  // "View entered".
  Inject(5, 5, EventPhase::kChange);
  Inject(0, 5, EventPhase::kChange);
  // Outside viewport.
  Inject(50, 0, EventPhase::kChange);  // "View exited".
  Inject(50, 50, EventPhase::kChange);
  Inject(0, 50, EventPhase::kChange);
  // Inside viewport.
  Inject(1, 1, EventPhase::kChange);  // "View entered".

  RunLoopUntil([&child_events] { return child_events.size() == 5u; });  // Succeeds or times out.

  {
    const auto& viewport_to_view_transform =
        child_events[0].view_parameters()->viewport_to_view_transform();
    {
      const auto& event = child_events[0];
      EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, 5.f, 0.f);
      ASSERT_TRUE(event.stream_info().has_value());
      EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
    }
    {
      const auto& event = child_events[1];
      EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, 5.f, 5.f);
      EXPECT_FALSE(event.stream_info().has_value());
    }
    {
      const auto& event = child_events[2];
      EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, 0.f, 5.f);
      EXPECT_FALSE(event.stream_info().has_value());
    }
    {
      const auto& event = child_events[3];
      EXPECT_FALSE(event.pointer_sample().has_value(), "Should get no pointer sample on View Exit");
      ASSERT_TRUE(event.stream_info().has_value());
      EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
    }
    {
      const auto& event = child_events[4];
      EXPECT_EQ_POINTER(event.pointer_sample(), viewport_to_view_transform, 1.f, 1.f);
      ASSERT_TRUE(event.stream_info().has_value());
      EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
    }
  }
}

TEST_F(FlatlandMouseIntegrationTest, InjectorDeath_ShouldCauseViewExitedEvent) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, {}, kIdentityMatrix);

  Inject(2.5f, 2.5f, EventPhase::kAdd);  // "View entered".

  // Register another injector, killing the old channel.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, {}, kIdentityMatrix);

  RunLoopUntil([&child_events] { return child_events.size() == 2u; });  // Succeeds or times out.

  {
    const auto& event = child_events[0];
    EXPECT_TRUE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
  }
  {
    const auto& event = child_events[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
}

TEST_F(FlatlandMouseIntegrationTest, REMOVEandCANCEL_ShouldCauseViewExitedEvents) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> child_mouse_source(
      std::move(child_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  auto child_view_ref = CreateAndAddChildView(*root_instance_,
                                              /*viewport_transform_id*/ kViewportTransform,
                                              /*parent_of_viewport_transform*/ kRootTransform,
                                              /*parent_content_id*/ ContentId(1), child_instance,
                                              std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  StartWatchLoop(child_mouse_source, child_events);

  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, {}, kIdentityMatrix);

  Inject(2.5f, 2.5f, EventPhase::kAdd);     // "View entered".
  Inject(2.5f, 2.5f, EventPhase::kRemove);  // "View exited".

  RunLoopUntil([&child_events] { return child_events.size() == 2u; });  // Succeeds or times out.

  {
    const auto& event = child_events[0];
    EXPECT_TRUE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
  }
  {
    const auto& event = child_events[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }

  child_events.clear();
  Inject(2.5f, 2.5f, EventPhase::kAdd);     // "View entered".
  Inject(2.5f, 2.5f, EventPhase::kCancel);  // "View exited".

  RunLoopUntil([&child_events] { return child_events.size() == 2u; });  // Succeeds or times out.

  {
    const auto& event = child_events[0];
    EXPECT_TRUE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kEntered);
  }
  {
    const auto& event = child_events[1];
    EXPECT_FALSE(event.pointer_sample().has_value());
    ASSERT_TRUE(event.stream_info().has_value());
    EXPECT_EQ(event.stream_info()->status(), MouseViewStatus::kExited);
  }
}

// Set up the following view hierarchy:
//    root    - context view
//     |
//   parent   - target view
//     |
//   child (anonymous)
//     |
//  granchild
//
// All views have fullscreen hit regions, and each subsequent view covers its parent.
// Observe that the anonymous view and its child do not get events or show up in hit tests (and
// block other views from getting events.)
TEST_F(FlatlandMouseIntegrationTest, AnonymousSubtree) {
  std::unique_ptr<FlatlandClientWithEventHandler> parent_instance;
  auto [parent_mouse_source_client_end, parent_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> parent_mouse_source(
      std::move(parent_mouse_source_client_end), dispatcher(), FailOnClose("Mouse source closed"));

  const auto parent_view_ref =
      CreateAndAddChildView(*root_instance_,
                            /*viewport_transform_id*/ kViewportTransform,
                            /*parent_of_viewport_transform*/ kRootTransform,
                            /*parent_content_id*/ ContentId(1), parent_instance,
                            std::move(parent_mouse_source_server_end));

  auto child_instance = std::make_unique<FlatlandClientWithEventHandler>(
      ConnectIntoRealm<fuc::Flatland>(), dispatcher());
  child_instance->set_on_close(FailOnClose("Lost connection to Scenic"));

  {
    // Set up the anonymous child view.
    auto [child_token, parent_token] = scenic::cpp::ViewCreationTokenPair::New();
    auto [parent_viewport_watcher_client_end, parent_viewport_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ParentViewportWatcher>().value();
    EXPECT_TRUE((*child_instance)
                    ->CreateView({{.token = std::move(child_token),
                                   .parent_viewport_watcher =
                                       std::move(parent_viewport_watcher_server_end)}})
                    .is_ok());
    EXPECT_TRUE((*child_instance)->CreateTransform({{.transform_id = kRootTransform}}).is_ok());
    EXPECT_TRUE((*child_instance)->SetRootTransform({{.transform_id = kRootTransform}}).is_ok());
    BlockingPresent(this, *child_instance);

    // Attach it to the parent.
    const ContentId parent_content_id(1);
    auto [child_view_watcher_client_end, child_view_watcher_server_end] =
        fidl::CreateEndpoints<fuc::ChildViewWatcher>().value();
    ViewportProperties properties;
    properties.logical_size(SizeU(kDefaultSize, kDefaultSize));
    EXPECT_TRUE(
        (*parent_instance)->CreateTransform({{.transform_id = kViewportTransform}}).is_ok());
    EXPECT_TRUE(
        (*parent_instance)
            ->CreateViewport({{.viewport_id = parent_content_id,
                               .token = std::move(parent_token),
                               .properties = std::move(properties),
                               .child_view_watcher = std::move(child_view_watcher_server_end)}})
            .is_ok());
    EXPECT_TRUE(
        (*parent_instance)
            ->SetContent({{.transform_id = kViewportTransform, .content_id = parent_content_id}})
            .is_ok());
    EXPECT_TRUE((*parent_instance)
                    ->AddChild({{.parent_transform_id = kRootTransform,
                                 .child_transform_id = kViewportTransform}})
                    .is_ok());
    BlockingPresent(this, *parent_instance);
  }

  // Create the named grandchild view along with its mouse source and attach it to the child.
  std::unique_ptr<FlatlandClientWithEventHandler> grandchild_instance;
  auto [grandchild_mouse_source_client_end, grandchild_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSource>().value();
  SimpleWatcherClient<fup::MouseSource> grandchild_mouse_source(
      std::move(grandchild_mouse_source_client_end), dispatcher(),
      FailOnClose("Mouse source closed"));

  CreateAndAddChildView(*child_instance,
                        /*viewport_transform_id=*/kViewportTransform, kRootTransform,
                        /*parent_content_id=*/ContentId(2), grandchild_instance,
                        std::move(grandchild_mouse_source_server_end));

  // Listen for mouse events.
  std::vector<MouseEvent> parent_events;
  StartWatchLoop(parent_mouse_source, parent_events);
  std::vector<MouseEvent> grandchild_events;
  StartWatchLoop(grandchild_mouse_source, grandchild_events);

  // Inject an input event at (0,0) which should hit every view. The anonymous child tree should be
  // ignored and the parent should receive it.
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(parent_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, {}, kIdentityMatrix);
  Inject(0, 0, EventPhase::kAdd);
  RunLoopUntil([&parent_events] { return parent_events.size() == 1u; });
  EXPECT_TRUE(parent_events[0].pointer_sample().has_value());
  EXPECT_TRUE(grandchild_events.empty());
}

TEST_F(FlatlandMouseIntegrationTest, MouseSourceV2_BasicInputTest) {
  std::unique_ptr<FlatlandClientWithEventHandler> child_instance;
  auto [child_mouse_source_client_end, child_mouse_source_server_end] =
      fidl::CreateEndpoints<fup::MouseSourceV2>().value();

  auto child_view_ref = CreateAndAddChildViewV2(*root_instance_,
                                                /*viewport_transform_id*/ kViewportTransform,
                                                /*parent_of_viewport_transform*/ kRootTransform,
                                                /*parent_content_id*/ ContentId(1), child_instance,
                                                std::move(child_mouse_source_server_end));

  // Listen for input events.
  std::vector<MouseEvent> child_events;
  MouseSourceV2Client child_mouse_source(std::move(child_mouse_source_client_end), dispatcher(),
                                         child_events);

  const std::vector<uint8_t> button_vec = {1};
  RegisterInjector(scenic::cpp::CloneViewRef(root_view_ref_),
                   scenic::cpp::CloneViewRef(child_view_ref),
                   DispatchPolicy::kMouseHoverAndLatchInTarget, button_vec, kIdentityMatrix);

  // Inject mouse move at (0, 0).
  Inject(0, 0, EventPhase::kAdd);
  RunLoopUntil([&child_events] { return !child_events.empty(); });

  EXPECT_EQ(child_events.size(), 1u);
  ASSERT_TRUE(child_events[0].pointer_sample().has_value());
  ASSERT_TRUE(child_events[0].stream_info().has_value());
  EXPECT_EQ(child_events[0].stream_info()->status(), MouseViewStatus::kEntered);
  EXPECT_EQ_POINTER(child_events[0].pointer_sample(), kIdentityMatrix, 0, 0);

  // Inject mouse press with button 1.
  Inject(0, 0, EventPhase::kChange, button_vec);
  RunLoopUntil([&child_events] { return child_events.size() == 2u; });

  EXPECT_EQ(child_events.size(), 2u);
  ASSERT_TRUE(child_events[1].pointer_sample().has_value());
  EXPECT_EQ_POINTER_WITH_BUTTONS(child_events[1].pointer_sample(), kIdentityMatrix, 0, 0,
                                 button_vec);
}

}  // namespace integration_tests
