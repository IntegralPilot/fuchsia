// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_V2_H_
#define SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_V2_H_

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async/default.h>
#include <zircon/status.h>

#include <deque>

#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/mouse_source_base.h"

namespace scenic_impl::input {

class MouseSourceV2 : public MouseSourceBase,
                      public fidl::Server<fuchsia_ui_pointer::MouseSourceV2> {
 public:
  static constexpr uint64_t kMaxUnacknowledgedEvents =
      fuchsia_ui_pointer::kMouseSourceV2MaxUnacknowledgedEvents;
  static constexpr size_t kMaxBufferedEvents = 1000;

  MouseSourceV2(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source,
                fit::function<void()> error_handler,
                async_dispatcher_t* dispatcher = async_get_default_dispatcher());

  ~MouseSourceV2() override = default;

  // |MouseSourceBase|
  void UpdateStream(StreamId stream_id, InternalMouseEvent event,
                    view_tree::BoundingBox view_bounds, bool view_exit) override;

  // |fidl::Server<fuchsia_ui_pointer::MouseSourceV2>|
  void AcknowledgeEvents(AcknowledgeEventsRequest& request,
                         AcknowledgeEventsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_ui_pointer::MouseSourceV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  void CloseChannel(zx_status_t epitaph);

 protected:
  // |MouseSourceBase|
  void PushEvent(fuchsia_ui_pointer::MouseEvent event) override;

 private:
  struct EventWithStamp {
    fuchsia_ui_pointer::MouseEvent event;
    uint64_t stamp = 0;
  };

  void QueueEvent(fuchsia_ui_pointer::MouseEvent event);
  void FlushPendingEvents();

  fidl::ServerBinding<fuchsia_ui_pointer::MouseSourceV2> binding_;

  bool is_closed_ = false;

  uint64_t total_events_created_ = 0;
  uint64_t last_sent_stamp_ = 0;
  uint64_t last_ack_stamp_ = 0;

  std::deque<EventWithStamp> event_buffer_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_MOUSE_SOURCE_V2_H_
