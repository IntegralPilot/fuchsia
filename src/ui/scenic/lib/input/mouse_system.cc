// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/mouse_system.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/mouse_source.h"
#include "src/ui/scenic/lib/input/mouse_source_v2.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/math.h"

#include <glm/glm.hpp>

namespace scenic_impl::input {

MouseSystem::MouseSystem(HitTester& hit_tester, RequestFocusFunc request_focus)
    : hit_tester_(hit_tester), request_focus_(std::move(request_focus)) {}

void MouseSystem::RegisterMouseSource(
    fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source_server_end,
    zx_koid_t client_view_ref_koid) {
  TRACE_DURATION("input", "MouseSystem::RegisterMouseSource");
  utils::CheckIsOnInputThread();
  const auto [it, success] = mouse_sources_.emplace(
      client_view_ref_koid,
      std::make_unique<MouseSource>(std::move(mouse_source_server_end),
                                    /*error_handler*/ [this, client_view_ref_koid] {
                                      mouse_sources_.erase(client_view_ref_koid);
                                    }));
  FX_DCHECK(success);
}

void MouseSystem::RegisterMouseSourceV2(
    fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source_server_end,
    zx_koid_t client_view_ref_koid) {
  TRACE_DURATION("input", "MouseSystem::RegisterMouseSourceV2");
  utils::CheckIsOnInputThread();
  const auto [it, success] = mouse_sources_.emplace(
      client_view_ref_koid,
      std::make_unique<MouseSourceV2>(std::move(mouse_source_server_end),
                                      /*error_handler*/ [this, client_view_ref_koid] {
                                        mouse_sources_.erase(client_view_ref_koid);
                                      }));
  FX_DCHECK(success);
}

void MouseSystem::SendEventToMouse(const view_tree::Snapshot& snapshot, zx_koid_t receiver,
                                   InternalMouseEvent event, StreamId stream_id, bool view_exit) {
  const auto it = mouse_sources_.find(receiver);
  if (it != mouse_sources_.end()) {
    if (view_exit) {
      // Bounding box and correct transform does not matter on view exit (since we don't send any
      // pointer samples), and we are likely working with a broken ViewTree, so skip them.
      it->second->UpdateStream(stream_id, std::move(event), {}, view_exit);
    } else {
      it->second->UpdateStream(stream_id,
                               EventWithReceiverFromViewportTransform<InternalMouseEvent>(
                                   snapshot, std::move(event), receiver),
                               snapshot.view_tree.at(receiver).bounding_box, view_exit);
    }
  }
}

void MouseSystem::InjectMouseEventExclusive(InternalMouseEvent event, const StreamId stream_id,
                                            const view_tree::Snapshot& snapshot) {
  FX_DCHECK(snapshot.IsDescendant(event.target, event.context))
      << "Should never allow injection into broken scene graph";
  FX_DCHECK(!current_exclusive_mouse_receivers_.contains(stream_id) ||
            current_exclusive_mouse_receivers_.at(stream_id) == event.target);
  current_exclusive_mouse_receivers_[stream_id] = event.target;
  SendEventToMouse(snapshot, event.target, std::move(event), stream_id, /*view_exit=*/false);
}

void MouseSystem::InjectMouseEventHitTested(InternalMouseEvent event, const StreamId stream_id,
                                            const view_tree::Snapshot& snapshot) {
  FX_DCHECK(snapshot.IsDescendant(event.target, event.context))
      << "Should never allow injection into broken scene graph";
  // Grab the current mouse receiver or create a new one.
  MouseReceiver& mouse_receiver = current_mouse_receivers_[stream_id];

  // Unlatch a current latch if all buttons are released.
  const bool button_down = !event.buttons.pressed.empty();
  mouse_receiver.latched = mouse_receiver.latched && button_down;

  // If the scene graph breaks while latched -> send a "View Exited" event and invalidate the
  // receiver for the remainder of the latch.
  if (mouse_receiver.latched && !snapshot.IsDescendant(mouse_receiver.view_koid, event.target) &&
      mouse_receiver.view_koid != event.target) {
    SendEventToMouse(snapshot, mouse_receiver.view_koid, std::move(event), stream_id,
                     /*view_exit=*/true);
    mouse_receiver.view_koid = ZX_KOID_INVALID;
    return;
  }
  // If not latched, choose the current target by finding the top view.
  if (!mouse_receiver.latched) {
    const zx_koid_t top_koid = hit_tester_.TopHitTest(snapshot, event, /*semantic_hit_test*/ false);

    // Determine the currently hovered view. If it's different than previously, send the
    // previous one a "View Exited" event.
    if (mouse_receiver.view_koid != top_koid) {
      // Send a clone of the event without transferring any possible wake lease.
      SendEventToMouse(snapshot, mouse_receiver.view_koid, event.ShallowClone(), stream_id,
                       /*view_exit=*/true);
    }
    mouse_receiver.view_koid = top_koid;

    // Button down on an unlatched stream -> latch it to the top-most view.
    if (button_down) {
      mouse_receiver.latched = true;
      request_focus_(/*requester*/ event.context, /*request*/ mouse_receiver.view_koid, snapshot);
    }
  }

  // Finally, send the event to the hovered/latched view.
  SendEventToMouse(snapshot, mouse_receiver.view_koid, std::move(event), stream_id,
                   /*view_exit=*/false);
}

void MouseSystem::CancelMouseStream(StreamId stream_id) {
  zx_koid_t receiver = ZX_KOID_INVALID;
  {
    const auto it = current_mouse_receivers_.find(stream_id);
    if (it != current_mouse_receivers_.end()) {
      receiver = it->second.view_koid;
      current_mouse_receivers_.erase(it);
    }
  }
  {
    const auto it = current_exclusive_mouse_receivers_.find(stream_id);
    if (it != current_exclusive_mouse_receivers_.end()) {
      receiver = it->second;
      current_exclusive_mouse_receivers_.erase(it);
    }
  }

  const auto it = mouse_sources_.find(receiver);
  if (it != mouse_sources_.end()) {
    it->second->UpdateStream(stream_id, {}, {}, /*view_exit=*/true);
  }
}

}  // namespace scenic_impl::input
