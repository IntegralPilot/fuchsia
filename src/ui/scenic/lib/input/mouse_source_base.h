// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_BASE_H_
#define SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_BASE_H_

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/fit/function.h>

#include <queue>
#include <unordered_set>

#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/stream_id.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace scenic_impl::input {

// The base implementation for the |fuchsia_ui_pointer::MouseSource| interface and its
// augmentations.
class MouseSourceBase {
 public:
  MouseSourceBase(zx_koid_t channel_koid, fit::function<void(zx_status_t)> close_channel)
      : channel_koid_(channel_koid), close_channel_(std::move(close_channel)) {}

  virtual ~MouseSourceBase() = default;

  virtual void UpdateStream(StreamId stream_id, InternalMouseEvent event,
                            view_tree::BoundingBox view_bounds, bool view_exit);

  static fuchsia_ui_pointer::MousePointerSample NewPointerSample(const InternalMouseEvent& event);
  static fuchsia_ui_pointer::MouseEvent NewMouseEvent(const InternalMouseEvent& event);
  static void AddDeviceInfoToEvent(fuchsia_ui_pointer::MouseEvent& out_event,
                                   const InternalMouseEvent& event);
  static void AddStreamInfoToEvent(fuchsia_ui_pointer::MouseEvent& out_event,
                                   const InternalMouseEvent& event, bool view_entered);
  static void AddViewParametersToEvent(fuchsia_ui_pointer::MouseEvent& out_event,
                                       const Viewport& viewport,
                                       const view_tree::BoundingBox& view_bounds);
  static fuchsia_ui_pointer::MouseEvent NewViewExitEvent(const InternalMouseEvent& event);

  zx_koid_t channel_koid() const { return channel_koid_; }

 protected:
  void WatchBase(fit::function<void(std::vector<fuchsia_ui_pointer::MouseEvent>)> callback);

  // Pushes an event to be sent to the client. Default implementation queues the event for V1
  // hanging-get Watch() calls. Subclasses can override this to implement other delivery mechanisms.
  virtual void PushEvent(fuchsia_ui_pointer::MouseEvent event);

  // TODO(https://fxbug.dev/42149398): Add clean up methods for when streams end or devices go away.
  // When we know exactly what that will look like.

  // TODO(https://fxbug.dev/42159133): Implement ANR.

 private:
  void SendPendingIfWaiting();

  const zx_koid_t channel_koid_;

  // Closes the fidl channel. This triggers the destruction of the MouseSourceBase object through
  // the error handler set in InputSystem. NOTE: No further method calls or member accesses should
  // be made after close_channel_(), since they might be made on a destroyed object.
  fit::function<void(zx_status_t)> close_channel_;

  bool is_first_event_ = true;
  Viewport current_viewport_;
  view_tree::BoundingBox current_view_bounds_;

  // Events waiting to be sent to client. Sent in batches of up to
  // fuchsia_ui_pointer::kMouseMaxEvent events on each call to Watch().
  std::queue<fuchsia_ui_pointer::MouseEvent> pending_events_;
  fit::function<void(std::vector<fuchsia_ui_pointer::MouseEvent>)> pending_callback_ = nullptr;

  std::unordered_set<StreamId> tracked_streams_;

  std::unordered_set<uint32_t> tracked_devices_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_BASE_H_
