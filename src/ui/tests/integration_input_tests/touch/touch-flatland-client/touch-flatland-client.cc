// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.element/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.test.input/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/hlcpp_conversion.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fidl/cpp/channel.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <zircon/status.h>

#include <array>
#include <memory>
#include <utility>
#include <vector>

#include <src/lib/ui/flatland-frame-scheduling/src/simple_present.h>
#include <src/ui/testing/util/fidl_cpp_helpers.h>

namespace touch_flatland_client {

// Implementation of a simple scenic client using the Flatland API.
class TouchFlatlandClient final {
 public:
  explicit TouchFlatlandClient(async::Loop* loop)
      : loop_(loop), outgoing_directory_(loop_->dispatcher()) {
    FX_CHECK(loop_);

    ZX_ASSERT_OK(outgoing_directory_.ServeFromStartupInfo());

    auto touch_input_listener_connect =
        component::Connect<fuchsia_ui_test_input::TouchInputListener>();
    ZX_ASSERT_OK(touch_input_listener_connect);
    touch_input_listener_ =
        fidl::Client(std::move(touch_input_listener_connect.value()), loop_->dispatcher());

    auto factory_connect = component::Connect<fuchsia_ui_composition::FlatlandFactory>();
    ZX_ASSERT_OK(factory_connect);
    fidl::SyncClient factory(std::move(factory_connect.value()));
    auto [flatland_client_end, flatland_server_end] =
        fidl::CreateEndpoints<fuchsia_ui_composition::Flatland>().value();
    auto create_res = factory->CreateFlatland({{
        .server_end = std::move(flatland_server_end),
        .config = {},
    }});
    ZX_ASSERT(create_res.is_ok());

    flatland_connection_ = simple_present::FlatlandConnection::Create(
        loop_->dispatcher(), std::move(flatland_client_end), "TouchFlatlandClient");

    auto presenter_connect = component::Connect<fuchsia_element::GraphicalPresenter>();
    ZX_ASSERT_OK(presenter_connect);
    presenter_ = fidl::Client(std::move(presenter_connect.value()), loop_->dispatcher());
    PresentView();
  }

  void PresentView() {
    auto [touch_source_client, touch_source_server] =
        fidl::Endpoints<fuchsia_ui_pointer::TouchSource>::Create();
    touch_source_ = fidl::Client(std::move(touch_source_client), loop_->dispatcher());

    auto [parent_viewport_watcher_client, parent_viewport_watcher_server] =
        fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
    parent_viewport_watcher_ = fidl::SyncClient(std::move(parent_viewport_watcher_client));

    fuchsia_ui_views::ViewCreationToken child_token;
    fuchsia_ui_views::ViewportCreationToken parent_token;
    zx::channel::create(0, &parent_token.value(), &child_token.value());

    fuchsia_element::ViewSpec view_spec;
    view_spec.viewport_creation_token() = std::move(parent_token);
    fuchsia_element::GraphicalPresenterPresentViewRequest request;
    request.view_spec() = std::move(view_spec);
    presenter_->PresentView(std::move(request))
        .Then([](fidl::Result<fuchsia_element::GraphicalPresenter::PresentView>& result) {
          if (!result.is_ok()) {
            FX_LOGS(FATAL) << "PresentView failed";
          }
        });

    auto identity = fidl::HLCPPToNatural(scenic::NewViewIdentityOnCreation());
    fuchsia_ui_composition::ViewBoundProtocols protocols;
    protocols.touch_source(std::move(touch_source_server));
    fuchsia_ui_composition::FlatlandCreateView2Request create_view2_req;
    create_view2_req.token(std::move(child_token));
    create_view2_req.view_identity(std::move(identity));
    create_view2_req.protocols(std::move(protocols));
    create_view2_req.parent_viewport_watcher(std::move(parent_viewport_watcher_server));

    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->CreateView2(std::move(create_view2_req)));

    // Create a minimal scene after receiving the layout information.
    auto layout = parent_viewport_watcher_->GetLayout();

    ZX_ASSERT_OK(layout);
    auto layout_info = layout.value().info();

    FX_CHECK(layout_info.logical_size().has_value());
    FX_CHECK(layout_info.device_pixel_ratio().has_value());

    width_ = layout_info.logical_size()->width();
    height_ = layout_info.logical_size()->height();
    device_pixel_ratio_ = layout_info.device_pixel_ratio().value();
    FX_LOGS(INFO) << "Flatland cpp client received layout info: w=" << width_ << ", h=" << height_
                  << ", dpr=(" << device_pixel_ratio_.x() << "," << device_pixel_ratio_.y() << ")";
    CreateScene();

    // Listen for pointer events.
    touch_source_->Watch({}).Then([&](auto& res) {
      ZX_ASSERT_OK(res);
      Watch(std::move(res.value().events()));
    });

    auto test_app_status_listener_connect =
        component::Connect<fuchsia_ui_test_input::TestAppStatusListener>();
    ZX_ASSERT_OK(test_app_status_listener_connect);
    fidl::SyncClient test_app_status_listener(std::move(test_app_status_listener_connect.value()));
    ZX_ASSERT_OK(test_app_status_listener->ReportStatus(
        {fuchsia_ui_test_input::TestAppStatus::kHandlersRegistered}));
  }

 private:
  static constexpr std::array<std::array<float, 4>, 6> kColorsRgba = {
      {{255.f, 0.f, 0.f, 255.f},      // red
       {255.f, 128.f, 0.f, 255.f},    // orange
       {255.f, 255.f, 0.f, 255.f},    // yellow
       {0.f, 255.f, 0.f, 255.f},      // green
       {0.f, 0.f, 255.f, 255.f},      // blue
       {128.f, 0.f, 255.f, 255.f}}};  // purple

  static const uint64_t kRootTransformId = 1;
  static const uint64_t kRectId = 1;
  static const uint64_t kRectTransformId = 2;

  // Creates a minimal scene containing a solid filled rectangle of size |width_| * |height_|.
  // Called after receiving layout info from
  // |fuchsia.ui.composition.ParentViewportWatcher.GetLayout|.
  void CreateScene() {
    fuchsia_ui_composition::TransformId rootTransformId(kRootTransformId);
    fuchsia_ui_composition::ContentId rectId(kRectId);
    fuchsia_ui_composition::TransformId rectTransformId = {kRectTransformId};

    // Create the root transform
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->CreateTransform({rootTransformId}));
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->SetRootTransform({rootTransformId}));

    // Create the transform for the rectangle.
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->CreateTransform({rectTransformId}));
    ZX_ASSERT_OK(
        flatland_connection_->FlatlandClient()->SetTranslation({{rectTransformId, {0, 0}}}));

    // Connect the transform to the scene graph.
    ZX_ASSERT_OK(
        flatland_connection_->FlatlandClient()->AddChild({rootTransformId, rectTransformId}));

    // Create the content and attach it to the transform.
    auto color = kColorsRgba[color_index_];
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->CreateFilledRect({rectId}));
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->SetSolidFill(
        {kRectId,
         {color[0] / 255.f, color[1] / 255.f, color[2] / 255.f, color[3] / 255.f},
         {width_, height_}}));
    ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->SetContent({kRectTransformId, kRectId}));

    Present();
  }

  void Present() {
    flatland_connection_->Present({}, [](auto) {});
  }

  // Creates a watch loop to continuously watch for pointer events using the
  // |fuchsia.ui.pointer.TouchSource.Watch|. Changes the color of the rectangle in the scene when
  // a tap event is received.
  void Watch(std::vector<fuchsia_ui_pointer::TouchEvent> events) {
    // Stores the response for touch events in |events|.
    std::vector<fuchsia_ui_pointer::TouchResponse> responses;
    for (fuchsia_ui_pointer::TouchEvent& event : events) {
      if (event.interaction_result().has_value() &&
          event.interaction_result()->status() ==
              fuchsia_ui_pointer::TouchInteractionStatus::kGranted) {
        interaction_granted_ = true;

        // Report any queued events now that the interaction has been granted.
        for (fuchsia_ui_pointer::TouchEvent& pending_event : pending_events_before_granted_) {
          ReportEvent(pending_event);
        }
        pending_events_before_granted_.clear();
      }

      fuchsia_ui_pointer::TouchResponse response;
      if (!HasValidatedTouchSample(event)) {
        responses.push_back(std::move(response));
        continue;
      }

      // Report the touch event only if the interaction has been granted.
      // If the interaction is not yet granted, queue the event for later.
      if (interaction_granted_) {
        ReportEvent(event);
      } else {
        pending_events_before_granted_.push_back(std::move(event));
      }

      response.response_type(fuchsia_ui_pointer::TouchResponseType::kYes);
      responses.push_back(std::move(response));
    }

    // Start next touch event watch.
    touch_source_->Watch({std::move(responses)}).Then([&](auto& res) {
      ZX_ASSERT_OK(res);
      Watch(std::move(res.value().events()));
    });
  }

  void ReportEvent(fuchsia_ui_pointer::TouchEvent& event) {
    FX_CHECK(interaction_granted_) << "only report events with interaction status of granted";
    const auto& pointer_sample = event.pointer_sample();

    // Store the view parameters received from a TouchEvent when either a new connection was
    // formed or the view parameters were modified.
    if (event.view_parameters().has_value()) {
      view_params_ = std::move(event.view_parameters());
    }

    if (pointer_sample->phase() == fuchsia_ui_pointer::EventPhase::kAdd) {
      // Change the color of the rectangle on a tap event.
      color_index_ = (color_index_ + 1) % kColorsRgba.size();
      auto color = kColorsRgba[color_index_];
      ZX_ASSERT_OK(flatland_connection_->FlatlandClient()->SetSolidFill(
          {kRectId,
           {color[0] / 255.f, color[1] / 255.f, color[2] / 255.f, color[3] / 255.f},
           {width_, height_}}));
      Present();
    }
    fuchsia_ui_test_input::TouchInputListenerReportTouchInputRequest request;
    // Only report ADD/CHANGE/REMOVE events for minimality; add more if necessary.
    if (pointer_sample->phase() == fuchsia_ui_pointer::EventPhase::kAdd ||
        pointer_sample->phase() == fuchsia_ui_pointer::EventPhase::kChange ||
        pointer_sample->phase() == fuchsia_ui_pointer::EventPhase::kRemove) {
      auto logical = ViewportToViewCoordinates(pointer_sample->position_in_viewport().value(),
                                               view_params_->viewport_to_view_transform());

      // The raw pointer event's coordinates are in pips (logical pixels). The test
      // expects coordinates in physical pixels. The former is transformed into the
      // latter with the DPR values received from |GetLayout|.
      request.local_x(logical[0] * device_pixel_ratio_.x());
      request.local_y(logical[1] * device_pixel_ratio_.y());
      request.phase(pointer_sample->phase());
      request.time_received(zx_clock_get_monotonic());
      request.component_name("touch-flatland-client");
      ZX_ASSERT_OK(touch_input_listener_->ReportTouchInput(request));
    }

    // Reset |interaction_granted_| as the current interaction has ended.
    if (pointer_sample->phase() == fuchsia_ui_pointer::EventPhase::kRemove) {
      interaction_granted_ = false;
      pending_events_before_granted_.clear();
    }
  }

  static bool HasValidatedTouchSample(const fuchsia_ui_pointer::TouchEvent& event) {
    if (!event.pointer_sample().has_value()) {
      return false;
    }
    FX_CHECK(event.pointer_sample()->interaction().has_value()) << "API guarantee";
    FX_CHECK(event.pointer_sample()->phase().has_value()) << "API guarantee";
    FX_CHECK(event.pointer_sample()->position_in_viewport().has_value()) << "API guarantee";
    return true;
  }

  static std::array<float, 2> ViewportToViewCoordinates(
      std::array<float, 2> viewport_coordinates,
      const std::array<float, 9>& viewport_to_view_transform) {
    // The transform matrix is a FIDL array with matrix data in column-major
    // order. For a matrix with data [a b c d e f g h i], and with the viewport
    // coordinates expressed as homogeneous coordinates, the logical view
    // coordinates are obtained with the following formula:
    //   |a d g|   |x|   |x'|
    //   |b e h| * |y| = |y'|
    //   |c f i|   |1|   |w'|
    // which we then normalize based on the w component:
    //   if w' not zero: (x'/w', y'/w')
    //   else (x', y')
    const auto& M = viewport_to_view_transform;
    const float x = viewport_coordinates[0];
    const float y = viewport_coordinates[1];
    const float xp = (M[0] * x) + (M[3] * y) + M[6];
    const float yp = (M[1] * x) + (M[4] * y) + M[7];
    const float wp = (M[2] * x) + (M[5] * y) + M[8];
    if (wp != 0) {
      return {xp / wp, yp / wp};
    }
    return {xp, yp};
  }

  // The main thread's message loop.
  async::Loop* loop_ = nullptr;

  // This holds exposed protocols.
  component::OutgoingDirectory outgoing_directory_;

  // Protocols used by this component.
  fidl::Client<fuchsia_ui_test_input::TouchInputListener> touch_input_listener_;

  fidl::Client<fuchsia_element::GraphicalPresenter> presenter_;

  std::unique_ptr<simple_present::FlatlandConnection> flatland_connection_;

  fidl::Client<fuchsia_ui_pointer::TouchSource> touch_source_;

  fidl::SyncClient<fuchsia_ui_composition::ParentViewportWatcher> parent_viewport_watcher_;

  // The fuchsia.ui.pointer.TouchSource protocol issues channel-global view
  // parameters on connection and on change. Events must apply these view
  // parameters to correctly map to logical view coordinates. The "nullopt"
  // state represents the absence of view parameters, early in the protocol
  // lifecycle.
  std::optional<fuchsia_ui_pointer::ViewParameters> view_params_;

  uint32_t color_index_ = 0;

  // Logical width and height of the view received from
  // |fuchsia.ui.composition.ParentViewportWatcher.GetLayout|.
  uint32_t width_ = 0;
  uint32_t height_ = 0;

  // DPR received from |fuchsia.ui.composition.ParentViewportWatcher.GetLayout|.
  fuchsia_math::VecF device_pixel_ratio_ = fuchsia_math::VecF(1.f, 1.f);

  // Indicates whether the latest touch interaction has been granted to the client.
  bool interaction_granted_ = false;

  // Any events that have been received before the touch has been granted;
  // these will be processed once the gesture has been granted to the client.
  std::vector<fuchsia_ui_pointer::TouchEvent> pending_events_before_granted_;
};
}  // namespace touch_flatland_client

int main(int argc, char** argv) {
  FX_LOGS(INFO) << "Starting Flatland cpp client";
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  touch_flatland_client::TouchFlatlandClient client(&loop);

  return loop.Run();
}
