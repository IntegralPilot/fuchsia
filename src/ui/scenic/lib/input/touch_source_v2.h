// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_V2_H_
#define SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_V2_H_

#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <zircon/status.h>

#include <deque>

#include "src/lib/fxl/macros.h"
#include "src/ui/scenic/lib/input/gesture_contender_inspector.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"
#include "src/ui/scenic/lib/input/touch_source_base.h"

namespace scenic_impl::input {

class TouchSourceV2 : public TouchSourceBase,
                      public fidl::Server<fuchsia_ui_pointer::TouchSourceV2> {
 public:
  static constexpr uint64_t kMaxUnacknowledgedEvents =
      fuchsia_ui_pointer::kTouchSourceV2MaxUnacknowledgedEvents;
  static constexpr size_t kMaxBufferedEvents = 1000;

  TouchSourceV2(async_dispatcher_t* dispatcher, zx_koid_t view_ref_koid,
                fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source,
                fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
                fit::function<void()> error_handler, GestureContenderInspector& inspector);

  ~TouchSourceV2() override = default;

  // |GestureContender|
  void UpdateStream(const view_tree::Snapshot& snapshot, StreamId stream_id,
                    InternalTouchEvent event, bool is_end_of_stream,
                    view_tree::BoundingBox view_bounds) override;

  // |fidl::Server<fuchsia_ui_pointer::TouchSourceV2>|
  void AcknowledgeEvents(AcknowledgeEventsRequest& request,
                         AcknowledgeEventsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_ui_pointer::TouchSourceV2> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 protected:
  // |TouchSourceBase|
  void CloseChannel(zx_status_t epitaph) override;
  void PushEvent(StreamId stream_id, AugmentedTouchEvent event) override;

 private:
  struct EventWithStamp {
    fuchsia_ui_pointer::TouchEvent event;
    uint64_t stamp = 0;
  };

  void QueueEvent(fuchsia_ui_pointer::TouchEvent event);
  void FlushPendingEvents();

  fidl::ServerBinding<fuchsia_ui_pointer::TouchSourceV2> binding_;

  const std::vector<GestureResponse> always_yes_response_ = {GestureResponse::kYes};

  bool is_closed_ = false;
  uint64_t total_events_created_ = 0;
  uint64_t last_sent_stamp_ = 0;
  uint64_t last_ack_stamp_ = 0;

  std::deque<EventWithStamp> event_buffer_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_TOUCH_SOURCE_V2_H_
