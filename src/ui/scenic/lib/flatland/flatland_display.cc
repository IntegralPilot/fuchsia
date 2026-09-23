// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/flatland_display.h"

#include <lib/async/default.h>
#include <lib/ui/scenic/cpp/view_identity.h>

#include "src/ui/scenic/lib/utils/logging.h"

#include <glm/glm.hpp>
#include <glm/gtc/constants.hpp>
#include <glm/gtc/type_ptr.hpp>
#include <glm/gtx/matrix_transform_2d.hpp>

static void ReportError() {
  // TODO(https://fxbug.dev/42157006): investigate how to propagate errors back to clients.
  // TODO(https://fxbug.dev/42156567): OK to crash until we have error propagation?  Probably so:
  // better that clients get feedback that they've done something wrong.  These are all in-tree
  // clients, anyway.
  FX_CHECK(false) << "Crashing on error.";
}

namespace flatland {

std::shared_ptr<FlatlandDisplay> FlatlandDisplay::New(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder,
    fidl::ServerEnd<fuchsia_ui_composition::FlatlandDisplay> server_end,
    scheduling::SessionId session_id, std::shared_ptr<display::Display> display,
    std::function<void()> destroy_display_function,
    std::shared_ptr<FlatlandPresenter> flatland_presenter, std::shared_ptr<LinkSystem> link_system,
    std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue) {
  auto flatland_display = std::shared_ptr<FlatlandDisplay>(new FlatlandDisplay(
      dispatcher_holder, session_id, std::move(display), std::move(destroy_display_function),
      std::move(flatland_presenter), std::move(link_system), std::move(uber_struct_queue)));

  // Natural FIDL bindings must be created and deleted on the same thread that it handles messages.
  async::PostTask(dispatcher_holder->dispatcher(),
                  [flatland_display, server_end = std::move(server_end)]() mutable {
                    flatland_display->Bind(std::move(server_end));
                  });

  return flatland_display;
}

FlatlandDisplay::FlatlandDisplay(
    std::shared_ptr<utils::DispatcherHolder> dispatcher_holder, scheduling::SessionId session_id,
    std::shared_ptr<display::Display> display, std::function<void()> destroy_display_function,
    std::shared_ptr<FlatlandPresenter> flatland_presenter, std::shared_ptr<LinkSystem> link_system,
    std::shared_ptr<UberStructSystem::UberStructQueue> uber_struct_queue)
    : dispatcher_holder_(std::move(dispatcher_holder)),
      session_id_(session_id),
      display_(std::move(display)),
      destroy_display_function_(std::move(destroy_display_function)),
      flatland_presenter_(std::move(flatland_presenter)),
      link_system_(std::move(link_system)),
      uber_struct_queue_(std::move(uber_struct_queue)),
      transform_graph_(session_id),
      root_transform_(transform_graph_.CreateTransform()) {
  FX_DCHECK(session_id_);
  FX_DCHECK(flatland_presenter_);
  FX_DCHECK(link_system_);
  FX_DCHECK(uber_struct_queue_);

  FX_LOGS(INFO) << "FlatlandDisplay NEW session_id=" << session_id_;
}

void FlatlandDisplay::Bind(fidl::ServerEnd<fuchsia_ui_composition::FlatlandDisplay> server_end) {
  FX_DCHECK(!binding_.has_value());
  binding_.emplace(dispatcher(), std::move(server_end), this,
                   std::mem_fn(&FlatlandDisplay::OnFidlClosed));
}

FlatlandDisplay::~FlatlandDisplay() {
  // Unlike Flatland, FlatlandDisplay doesn't manage any client images, therefore doesn't need to
  // pass a release fence to RemoveSession().
  flatland_presenter_->RemoveSession(session_id_, std::nullopt);

  FX_LOGS(INFO) << "FlatlandDisplay DESTROYED session_id=" << session_id_;
}

void FlatlandDisplay::OnFidlClosed(fidl::UnbindInfo unbind_info) {
  if (!unbind_info.is_user_initiated()) {
    FX_LOGS(INFO) << "FlatlandDisplay::OnFidlClosed() session_id=" << session_id_
                  << " because: " << unbind_info.FormatDescription();
  }
  binding_.reset();
  destroy_display_function_();
}

void FlatlandDisplay::SetContent(SetContentRequest& request, SetContentCompleter::Sync& completer) {
  SetContent(std::move(request.token()), std::move(request.child_view_watcher()));
}

void FlatlandDisplay::SetContent(
    fuchsia_ui_views::ViewportCreationToken token,
    fidl::ServerEnd<fuchsia_ui_composition::ChildViewWatcher> child_view_watcher) {
  // Attempting to link with an invalid token will never succeed, so its better to fail early and
  // immediately close the link connection.
  if (!token.value().is_valid()) {
    FX_LOGS(ERROR) << "CreateViewport failed, ViewportCreationToken was invalid";
    ReportError();
    return;
  }

  if (link_to_child_) {
    const bool child_removed = transform_graph_.RemoveChild(link_to_child_->parent_transform_handle,
                                                            link_to_child_->internal_link_handle);
    FX_DCHECK(child_removed);

    const bool content_released =
        transform_graph_.ReleaseTransform(link_to_child_->parent_transform_handle);
    FX_DCHECK(content_released);

    link_to_child_.reset();
  }

  auto child_transform = transform_graph_.CreateTransform();

  const auto device_pixel_ratio = display_->device_pixel_ratio();
  FX_LOGS(INFO) << "Device Pixel Ratio: " << device_pixel_ratio.x << "x" << device_pixel_ratio.y;

  fuchsia_ui_composition::ViewportProperties properties;
  {
    // To calculate the logical size, we need to divide the physical pixel size by the DPR.
    properties.logical_size(fuchsia_math::SizeU{{
        .width = static_cast<uint32_t>(static_cast<float>(display_->width_in_px()) /
                                       device_pixel_ratio.x),
        .height = static_cast<uint32_t>(static_cast<float>(display_->height_in_px()) /
                                        device_pixel_ratio.y),
    }});
    properties.inset(fuchsia_math::Inset{{.top = 0, .right = 0, .bottom = 0, .left = 0}});
  }

  // We can initialize the Link importer immediately, since no state changes actually occur before
  // the feed-forward portion of this method. We also forward the initial ViewportProperties through
  // the LinkSystem immediately, so the child can receive them as soon as possible.
  // NOTE: clients won't receive CONNECTED_TO_DISPLAY until LinkSystem::UpdateLinkWatchers() is
  // called, typically during rendering.
  link_to_child_ = link_system_->CreateLinkToChild(
      dispatcher_holder_, std::move(token), std::move(properties), std::move(child_view_watcher),
      child_transform,
      [ref = weak_from_this(),
       dispatcher_holder = dispatcher_holder_](const std::string& error_log) {
        FX_CHECK(dispatcher_holder->dispatcher() == async_get_default_dispatcher())
            << "Link protocol error reported on the wrong dispatcher.";

        // TODO(https://fxbug.dev/42157006): FlatlandDisplay currently has no way to notify clients
        // of errors.
        FX_LOGS(ERROR) << "FlatlandDisplay illegal client usage: " << error_log;
      });
  FX_CHECK(child_transform == link_to_child_->parent_transform_handle);

  // This is the feed-forward portion of the method, i.e. the part which enqueues an updated
  // UberStruct.
  bool child_added;
  child_added = transform_graph_.AddChild(link_to_child_->parent_transform_handle,
                                          link_to_child_->internal_link_handle);
  FX_DCHECK(child_added);
  child_added = transform_graph_.AddChild(root_transform_, link_to_child_->parent_transform_handle);
  FX_DCHECK(child_added);

  auto uber_struct = std::make_unique<UberStruct>();
  uber_struct->debug_name = "FlatlandDisplay";

  // TODO(https://fxbug.dev/42156567): given this fixed topology, we probably don't need to use
  // ComputeAndCleanup(), we can just stamp something out based on a fixed template.
  // TODO(https://fxbug.dev/42116832): Decide on a proper limit on compute time for topological
  // sorting.
  auto data = transform_graph_.ComputeAndCleanup(
      root_transform_, std::numeric_limits<uint64_t>::max(), uber_struct->resource());
  FX_DCHECK(data.iterations != std::numeric_limits<uint64_t>::max());
  FX_DCHECK(data.sorted_transforms[0].handle == root_transform_);

  uber_struct->local_topology = std::move(data.sorted_transforms);

  uber_struct->local_clip_regions[link_to_child_->parent_transform_handle] = TransformClipRegion({
      .x = 0,
      .y = 0,
      .width = static_cast<int32_t>(display_->width_in_px()),
      .height = static_cast<int32_t>(display_->height_in_px()),
  });

  // By scaling the local matrix of the uberstruct here by the device pixel ratio, we ensure that
  // internally, the sizes of content on flatland instances that are hooked up to this display are
  // scaled up by the appropriate amount.
  uber_struct->local_matrices[link_to_child_->parent_transform_handle] =
      glm::scale(glm::mat3(), device_pixel_ratio);

  auto present_id = scheduling::GetNextPresentId();
  uber_struct_queue_->Push(present_id, std::move(uber_struct));
  flatland_presenter_->ScheduleUpdateForSession(zx::time(0), {session_id_, present_id},
                                                /*squashable=*/true, /*release_fences=*/{},
                                                /*release_counters=*/{},
                                                /*present_fences=*/{}, /*schedule_asap=*/false);
}

void FlatlandDisplay::SetDevicePixelRatio(SetDevicePixelRatioRequest& request,
                                          SetDevicePixelRatioCompleter::Sync& completer) {
  SetDevicePixelRatio(request.device_pixel_ratio());
}

void FlatlandDisplay::SetDevicePixelRatio(fuchsia_math::VecF device_pixel_ratio) {
  if (device_pixel_ratio.x() < 1.f || device_pixel_ratio.y() < 1.f) {
    FX_LOGS(ERROR) << "SetDevicePixelRatio failed, device_pixel_ratio is invalid";
    ReportError();
    return;
  }

  display_->set_device_pixel_ratio({device_pixel_ratio.x(), device_pixel_ratio.y()});
}

}  // namespace flatland
