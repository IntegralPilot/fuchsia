// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_source_v2.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <algorithm>

#include "src/lib/fsl/handles/object_info.h"

namespace scenic_impl::input {

TouchSourceV2::TouchSourceV2(
    async_dispatcher_t* dispatcher, zx_koid_t view_ref_koid,
    fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source,
    fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
    fit::function<void()> error_handler, GestureContenderInspector& inspector)
    : TouchSourceBase(fsl::GetKoid(touch_source.channel().get()), view_ref_koid, std::move(respond),
                      inspector),
      binding_(
          dispatcher, std::move(touch_source), this,
          [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
            if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
              FX_LOGS(ERROR) << "TouchSourceV2 fidl channel closed: " << info.FormatDescription();
            } else {
              FX_LOGS(INFO) << "TouchSourceV2 fidl channel closed: " << info.FormatDescription();
            }
            error_handler();
          }) {}

void TouchSourceV2::UpdateStream(const view_tree::Snapshot& snapshot, StreamId stream_id,
                                 InternalTouchEvent event, bool is_end_of_stream,
                                 view_tree::BoundingBox view_bounds) {
  if (is_closed_) {
    // Satisfy Gesture Arena response contract to prevent hang even if channel is closed.
    respond_(stream_id, {GestureResponse::kNo});
    return;
  }
  TRACE_DURATION("input", "TouchSourceV2::UpdateStream");

  TouchSourceBase::UpdateStream(snapshot, stream_id, std::move(event), is_end_of_stream,
                                view_bounds);

  // V2 client is always-consume; report kYes to gesture disambiguation after
  // the event has been enqueued into TouchSourceBase/ongoing_streams_.
  // This satisfies the GestureArena contract (expecting exactly one response per
  // event sample) while adapting to V2's always-consume semantics.
  respond_(stream_id, always_yes_response_);
}

void TouchSourceV2::PushEvent(StreamId stream_id, AugmentedTouchEvent event) {
  if (is_closed_) {
    return;
  }
  QueueEvent(std::move(event.touch_event));
}

void TouchSourceV2::AcknowledgeEvents(AcknowledgeEventsRequest& request,
                                      AcknowledgeEventsCompleter::Sync& completer) {
  if (is_closed_) {
    return;
  }
  uint64_t ack_stamp = request.last_acknowledged_event_stamp();
  if (ack_stamp <= last_ack_stamp_ || ack_stamp > last_sent_stamp_) {
    FX_LOGS(ERROR) << "TouchSourceV2: Invalid acknowledge stamp " << ack_stamp
                   << " (last_ack=" << last_ack_stamp_ << ", last_sent=" << last_sent_stamp_
                   << ").";
    CloseChannel(ZX_ERR_INVALID_ARGS);
    return;
  }

  last_ack_stamp_ = ack_stamp;
  FlushPendingEvents();
}

void TouchSourceV2::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_ui_pointer::TouchSourceV2> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "TouchSourceV2 received unknown method ordinal: " << metadata.method_ordinal;
}

void TouchSourceV2::CloseChannel(zx_status_t epitaph) {
  if (is_closed_) {
    return;
  }
  is_closed_ = true;
  event_buffer_.clear();
  FX_LOGS(WARNING) << "Closing TouchSourceV2 due to " << zx_status_get_string(epitaph);
  binding_.Close(epitaph);
}

void TouchSourceV2::QueueEvent(fuchsia_ui_pointer::TouchEvent event) {
  if (is_closed_) {
    return;
  }
  ++total_events_created_;
  event_buffer_.push_back(
      EventWithStamp{.event = std::move(event), .stamp = total_events_created_});
  FlushPendingEvents();
}

void TouchSourceV2::FlushPendingEvents() {
  if (is_closed_) {
    return;
  }
  while (!event_buffer_.empty() &&
         (last_sent_stamp_ - last_ack_stamp_) < kMaxUnacknowledgedEvents) {
    uint64_t available_credit = kMaxUnacknowledgedEvents - (last_sent_stamp_ - last_ack_stamp_);
    size_t batch_size = std::min<size_t>(
        {available_credit, event_buffer_.size(), fuchsia_ui_pointer::kTouchMaxEvent});
    if (batch_size == 0) {
      break;
    }

    std::vector<fuchsia_ui_pointer::TouchEvent> events;
    events.reserve(batch_size);
    uint64_t last_stamp = 0;
    for (size_t i = 0; i < batch_size; ++i) {
      last_stamp = event_buffer_.front().stamp;
      auto event = std::move(event_buffer_.front().event);
      if (event.trace_flow_id().has_value()) {
        // Touch events have their trace flow begun upstream in Injector, so step the flow here.
        TRACE_FLOW_STEP("input", "dispatch_event_to_client", event.trace_flow_id().value());
      }
      events.push_back(std::move(event));
      event_buffer_.pop_front();
    }

    inspector_.OnInjectedEvents(view_ref_koid_, events.size());

    last_sent_stamp_ = last_stamp;

    auto result = fidl::SendEvent(binding_)->OnTouchEvents(
        fuchsia_ui_pointer::TouchSourceV2OnTouchEventsRequest{
            {.events = std::move(events), .last_event_stamp = last_stamp}});
    if (!result.is_ok()) {
      FX_LOGS(ERROR) << "Failed to send OnTouchEvents: " << result.error_value();
      CloseChannel(result.error_value().status());
      return;
    }
  }

  if (event_buffer_.size() > kMaxBufferedEvents) {
    FX_LOGS(ERROR)
        << "TouchSourceV2: Client is not acknowledging events fast enough. Closing channel.";
    CloseChannel(ZX_ERR_BAD_STATE);
    return;
  }
}

}  // namespace scenic_impl::input
