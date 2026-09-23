// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_MOUSE_SYSTEM_H_
#define SRC_UI_SCENIC_LIB_INPUT_MOUSE_SYSTEM_H_

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <mutex>
#include <unordered_map>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/input/constants.h"
#include "src/ui/scenic/lib/input/helper.h"
#include "src/ui/scenic/lib/input/hit_tester.h"
#include "src/ui/scenic/lib/input/mouse_source_base.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace scenic_impl::input {

// Tracks input APIs.
//
// Thread-unsafe.  All methods are expected to be called from the same thread (the "input thread").
// The only exception is `SetViewTreeSnapshot()`, which replaces the existing snapshot under a mutex
// lock.
class MouseSystem {
 public:
  explicit MouseSystem(HitTester& hit_tester, RequestFocusFunc request_focus);
  ~MouseSystem() = default;

  void RegisterMouseSource(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source_server_end,
                           zx_koid_t client_view_ref_koid);

  void RegisterMouseSourceV2(
      fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source_server_end,
      zx_koid_t client_view_ref_koid);

  // Injects a mouse event directly to the View with koid |event.target|.
  void InjectMouseEventExclusive(InternalMouseEvent event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot);
  // Injects a mouse event by hit testing for appropriate targets.
  void InjectMouseEventHitTested(InternalMouseEvent event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot);
  // Sends a "View exit" event to the current receiver of |stream_id|, if there is one, and resets
  // the tracking state for the mouse stream.
  void CancelMouseStream(StreamId stream_id);

 private:
  // Locates and sends an event to the MouseSource identified by |receiver|, if one exists.
  void SendEventToMouse(const view_tree::Snapshot& snapshot, zx_koid_t receiver,
                        InternalMouseEvent event, StreamId stream_id, bool view_exit);

  /// Construction-time state.
  HitTester& hit_tester_;
  const RequestFocusFunc request_focus_;

  //// Mouse state
  // Struct for tracking the mouse state of a particular event stream.
  struct MouseReceiver {
    zx_koid_t view_koid = ZX_KOID_INVALID;
    bool latched = false;
  };
  // Currently hovered/latched view for each mouse stream.
  std::unordered_map<StreamId, MouseReceiver> current_mouse_receivers_;
  // Currently targeted mouse receiver for exclusive streams.
  std::unordered_map<StreamId, zx_koid_t> current_exclusive_mouse_receivers_;
  // All MouseSource instances. Each instance can be the receiver of any number of mouse streams,
  // and each stream is captured in either |current_mouse_receivers_| or
  // |current_exclusive_mouse_receivers_|.
  std::unordered_map<zx_koid_t, std::unique_ptr<MouseSourceBase>> mouse_sources_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_MOUSE_SYSTEM_H_
