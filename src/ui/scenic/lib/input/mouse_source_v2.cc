// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/mouse_source_v2.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <algorithm>

#include "src/lib/fsl/handles/object_info.h"

namespace scenic_impl::input {

MouseSourceV2::MouseSourceV2(fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source,
                             fit::function<void()> error_handler, async_dispatcher_t* dispatcher)
    : MouseSourceBase(fsl::GetKoid(mouse_source.channel().get()),
                      [this](zx_status_t epitaph) { CloseChannel(epitaph); }),
      binding_(
          dispatcher, std::move(mouse_source), this,
          [error_handler = std::move(error_handler)](fidl::UnbindInfo info) {
            if (info.status() != ZX_OK && info.status() != ZX_ERR_PEER_CLOSED) {
              FX_LOGS(ERROR) << "MouseSourceV2 fidl channel closed: " << info.FormatDescription();
            } else {
              FX_LOGS(INFO) << "MouseSourceV2 fidl channel closed: " << info.FormatDescription();
            }
            error_handler();
          }) {}

void MouseSourceV2::UpdateStream(const StreamId stream_id, InternalMouseEvent event,
                                 const view_tree::BoundingBox view_bounds, const bool view_exit) {
  if (is_closed_) {
    return;
  }
  TRACE_DURATION("input", "MouseSourceV2::UpdateStream");

  MouseSourceBase::UpdateStream(stream_id, std::move(event), view_bounds, view_exit);
}

void MouseSourceV2::PushEvent(fuchsia_ui_pointer::MouseEvent event) {
  if (is_closed_) {
    return;
  }
  QueueEvent(std::move(event));
}

void MouseSourceV2::AcknowledgeEvents(AcknowledgeEventsRequest& request,
                                      AcknowledgeEventsCompleter::Sync& completer) {
  if (is_closed_) {
    return;
  }
  uint64_t ack_stamp = request.last_acknowledged_event_stamp();
  if (ack_stamp <= last_ack_stamp_ || ack_stamp > last_sent_stamp_) {
    FX_LOGS(ERROR) << "MouseSourceV2: Invalid acknowledge stamp " << ack_stamp
                   << " (last_ack=" << last_ack_stamp_ << ", last_sent=" << last_sent_stamp_
                   << ").";
    CloseChannel(ZX_ERR_INVALID_ARGS);
    return;
  }

  last_ack_stamp_ = ack_stamp;
  FlushPendingEvents();
}

void MouseSourceV2::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_ui_pointer::MouseSourceV2> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "MouseSourceV2 received unknown method ordinal: " << metadata.method_ordinal;
}

void MouseSourceV2::CloseChannel(zx_status_t epitaph) {
  if (is_closed_) {
    return;
  }
  is_closed_ = true;
  event_buffer_.clear();
  FX_LOGS(WARNING) << "Closing MouseSourceV2 due to " << zx_status_get_string(epitaph);
  binding_.Close(epitaph);
}

void MouseSourceV2::QueueEvent(fuchsia_ui_pointer::MouseEvent event) {
  if (is_closed_) {
    return;
  }
  ++total_events_created_;
  event_buffer_.push_back(
      EventWithStamp{.event = std::move(event), .stamp = total_events_created_});
  FlushPendingEvents();
}

void MouseSourceV2::FlushPendingEvents() {
  if (is_closed_) {
    return;
  }
  while (!event_buffer_.empty() &&
         (last_sent_stamp_ - last_ack_stamp_) < kMaxUnacknowledgedEvents) {
    uint64_t available_credit = kMaxUnacknowledgedEvents - (last_sent_stamp_ - last_ack_stamp_);
    size_t batch_size = std::min<size_t>(
        {available_credit, event_buffer_.size(), fuchsia_ui_pointer::kMouseMaxEvent});
    if (batch_size == 0) {
      break;
    }

    std::vector<fuchsia_ui_pointer::MouseEvent> events;
    events.reserve(batch_size);
    uint64_t last_stamp = 0;
    for (size_t i = 0; i < batch_size; ++i) {
      last_stamp = event_buffer_.front().stamp;
      auto event = std::move(event_buffer_.front().event);
      if (event.trace_flow_id().has_value()) {
        // Mouse events generate a new nonce in MouseSourceBase::NewMouseEvent, so begin the flow
        // here.
        TRACE_FLOW_BEGIN("input", "dispatch_event_to_client", event.trace_flow_id().value());
      }
      events.push_back(std::move(event));
      event_buffer_.pop_front();
    }

    last_sent_stamp_ = last_stamp;

    auto result = fidl::SendEvent(binding_)->OnMouseEvents(
        fuchsia_ui_pointer::MouseSourceV2OnMouseEventsRequest{
            {.events = std::move(events), .last_event_stamp = last_stamp}});
    if (!result.is_ok()) {
      FX_LOGS(ERROR) << "Failed to send OnMouseEvents: " << result.error_value();
      CloseChannel(result.error_value().status());
      return;
    }
  }

  if (event_buffer_.size() > kMaxBufferedEvents) {
    FX_LOGS(ERROR)
        << "MouseSourceV2: Client is not acknowledging events fast enough. Closing channel.";
    CloseChannel(ZX_ERR_BAD_STATE);
    return;
  }
}

}  // namespace scenic_impl::input
