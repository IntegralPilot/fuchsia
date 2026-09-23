// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_source_base.h"

#include <lib/async/cpp/time.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <unordered_map>

#include "src/lib/fxl/macros.h"

namespace scenic_impl::input {

namespace {

GestureResponse ConvertToGestureResponse(fuchsia_ui_pointer::TouchResponseType type) {
  switch (type) {
    case fuchsia_ui_pointer::TouchResponseType::kYes:
      return GestureResponse::kYes;
    case fuchsia_ui_pointer::TouchResponseType::kYesPrioritize:
      return GestureResponse::kYesPrioritize;
    case fuchsia_ui_pointer::TouchResponseType::kNo:
      return GestureResponse::kNo;
    case fuchsia_ui_pointer::TouchResponseType::kMaybe:
      return GestureResponse::kMaybe;
    case fuchsia_ui_pointer::TouchResponseType::kMaybePrioritize:
      return GestureResponse::kMaybePrioritize;
    case fuchsia_ui_pointer::TouchResponseType::kMaybeSuppress:
      return GestureResponse::kMaybeSuppress;
    case fuchsia_ui_pointer::TouchResponseType::kMaybePrioritizeSuppress:
      return GestureResponse::kMaybePrioritizeSuppress;
    case fuchsia_ui_pointer::TouchResponseType::kHold:
      return GestureResponse::kHold;
    case fuchsia_ui_pointer::TouchResponseType::kHoldSuppress:
      return GestureResponse::kHoldSuppress;
    default:
      return GestureResponse::kUndefined;
  }
}

bool IsHold(GestureResponse response) {
  switch (response) {
    case GestureResponse::kHold:
    case GestureResponse::kHoldSuppress:
      return true;
    default:
      return false;
  }
}

bool IsHold(fuchsia_ui_pointer::TouchResponseType response) {
  switch (response) {
    case fuchsia_ui_pointer::TouchResponseType::kHold:
    case fuchsia_ui_pointer::TouchResponseType::kHoldSuppress:
      return true;
    default:
      return false;
  }
}

}  // namespace

fuchsia_ui_pointer::EventPhase TouchSourceBase::ConvertToEventPhase(Phase phase) {
  switch (phase) {
    case Phase::kAdd:
      return fuchsia_ui_pointer::EventPhase::kAdd;
    case Phase::kChange:
      return fuchsia_ui_pointer::EventPhase::kChange;
    case Phase::kRemove:
      return fuchsia_ui_pointer::EventPhase::kRemove;
    case Phase::kCancel:
      return fuchsia_ui_pointer::EventPhase::kCancel;
    default:
      // Never reached.
      FX_CHECK(false) << "Unknown phase: " << static_cast<int>(phase);
      return fuchsia_ui_pointer::EventPhase::kCancel;
  }
}

fuchsia_ui_pointer::TouchEvent TouchSourceBase::NewTouchEvent(StreamId stream_id,
                                                              const InternalTouchEvent& event) {
  fuchsia_ui_pointer::TouchEvent new_event;
  new_event.timestamp(event.timestamp);
  if (event.trace_flow_id.has_value()) {
    new_event.trace_flow_id(event.trace_flow_id.value());
  }

  {
    fuchsia_ui_pointer::TouchPointerSample pointer;

    pointer.phase(ConvertToEventPhase(event.phase));
    pointer.position_in_viewport(
        std::array<float, 2>{event.position_in_viewport[0], event.position_in_viewport[1]});
    pointer.interaction(fuchsia_ui_pointer::TouchInteractionId{
        {.device_id = event.device_id,
         .pointer_id = event.pointer_id,
         // The truncation from uint64_t to uint32_t is safe because stream collisions
         // only affect internal Scenic mapping. Clients only ever observe streams
         // related to their own views, which practically never exceed 2^32 concurrent touches.
         .interaction_id = static_cast<uint32_t>(stream_id)}});
    new_event.pointer_sample(std::move(pointer));
  }

  return new_event;
}

void TouchSourceBase::AddInteractionResultsToEvent(fuchsia_ui_pointer::TouchEvent& event,
                                                   StreamId stream_id, uint32_t device_id,
                                                   uint32_t pointer_id, const bool awarded_win) {
  event.interaction_result(fuchsia_ui_pointer::TouchInteractionResult{
      {.interaction =
           fuchsia_ui_pointer::TouchInteractionId{
               {.device_id = device_id,
                .pointer_id = pointer_id,
                // The truncation from uint64_t to uint32_t is safe because stream collisions
                // only affect internal Scenic mapping. Clients only ever observe streams
                // related to their own views, which practically never exceed 2^32 concurrent
                // touches.
                .interaction_id = static_cast<uint32_t>(stream_id)}},
       .status = awarded_win ? fuchsia_ui_pointer::TouchInteractionStatus::kGranted
                             : fuchsia_ui_pointer::TouchInteractionStatus::kDenied}});
}

fuchsia_ui_pointer::TouchEvent TouchSourceBase::NewEndEvent(StreamId stream_id, uint32_t device_id,
                                                            uint32_t pointer_id, bool awarded_win) {
  fuchsia_ui_pointer::TouchEvent new_event;
  new_event.timestamp(async::Now(async_get_default_dispatcher()).get());
  new_event.trace_flow_id(TRACE_NONCE());
  AddInteractionResultsToEvent(new_event, stream_id, device_id, pointer_id, awarded_win);
  return new_event;
}

void TouchSourceBase::AddViewParametersToEvent(fuchsia_ui_pointer::TouchEvent& event,
                                               const Viewport& viewport,
                                               view_tree::BoundingBox view_bounds) {
  const auto& [extents, _, receiver_from_viewport_transform] = viewport;
  FX_DCHECK(receiver_from_viewport_transform.has_value());
  event.view_parameters(fuchsia_ui_pointer::ViewParameters{{
      .view = fuchsia_ui_pointer::Rectangle{{.min = view_bounds.min, .max = view_bounds.max}},
      .viewport = fuchsia_ui_pointer::Rectangle{{.min = {extents.min[0], extents.min[1]},
                                                 .max = {extents.max[0], extents.max[1]}}},
      .viewport_to_view_transform = receiver_from_viewport_transform.value(),
  }});
}

TouchSourceBase::TouchSourceBase(
    zx_koid_t channel_koid, zx_koid_t view_ref_koid,
    fit::function<void(StreamId, const std::vector<GestureResponse>&)> respond,
    GestureContenderInspector& inspector)
    : GestureContender(view_ref_koid),
      channel_koid_(channel_koid),
      respond_(std::move(respond)),
      inspector_(inspector) {}

void TouchSourceBase::UpdateStream(const view_tree::Snapshot& snapshot, StreamId stream_id,
                                   InternalTouchEvent event, bool is_end_of_stream,
                                   view_tree::BoundingBox view_bounds) {
  TRACE_DURATION("input", "TouchSourceBase::UpdateStream");

  const bool is_new_stream = !ongoing_streams_.contains(stream_id);
  FX_CHECK(is_new_stream == (event.phase == Phase::kAdd))
      << "Stream must only start with ADD. stream_id: " << stream_id
      << " view_ref_koid: " << view_ref_koid_;
  FX_CHECK(is_end_of_stream == (event.phase == Phase::kRemove || event.phase == Phase::kCancel));

  if (is_new_stream) {
    ongoing_streams_.try_emplace(
        stream_id, StreamData{.device_id = event.device_id, .pointer_id = event.pointer_id});
  }
  auto& stream = ongoing_streams_.at(stream_id);
  FX_DCHECK(stream.device_id == event.device_id);
  FX_DCHECK(stream.pointer_id == event.pointer_id);

  {  // Build the event.
    AugmentedTouchEvent out_event;
    {
      out_event.touch_event = NewTouchEvent(stream_id, event);
      auto& touch_event = out_event.touch_event;

      FX_DCHECK(!(won_streams_awaiting_first_message_.contains(stream_id) && !is_new_stream))
          << "Can't have a pre-decided win for an ongoing stream.";
      if (is_new_stream) {
        // First time we see a device we need to add DeviceInfo to the message.
        if (!seen_devices_.contains(event.device_id)) {
          fuchsia_ui_pointer::TouchDeviceInfo device_info;
          device_info.id(event.device_id);
          touch_event.device_info(std::move(device_info));

          seen_devices_.emplace(event.device_id);
        }

        // If the stream was won before the first message arrived, attach the "win" to the first
        // message.
        if (won_streams_awaiting_first_message_.contains(stream_id)) {
          AddInteractionResultsToEvent(touch_event, stream_id, event.device_id, event.pointer_id,
                                       true);
          won_streams_awaiting_first_message_.erase(stream_id);
          stream.was_won = true;
        }
      }

      // Add ViewParameters to the message if the viewport or view bounds have changed (which is
      // always true for the first message).
      // (For cancel events it's likely we're not in the view tree, so we can't trust viewport
      //  transforms or view bounds. Skip checking them since it's not necessary at the end of a
      //  stream anyway.)
      if (event.phase != Phase::kCancel &&
          (current_viewport_ != event.viewport || current_view_bounds_ != view_bounds ||
           is_first_event_)) {
        is_first_event_ = false;
        current_viewport_ = event.viewport;
        current_view_bounds_ = view_bounds;
        AddViewParametersToEvent(touch_event, current_viewport_, current_view_bounds_);
      }
    }

    Augment(snapshot, out_event, event);
    if (event.wake_lease) {
      out_event.touch_event.wake_lease(std::move(event.wake_lease));
    }
    PushEvent(stream_id, std::move(out_event));
  }

  stream.stream_has_ended = is_end_of_stream;

  // Cleanup complete stream.
  if (is_end_of_stream && stream.was_won) {
    ongoing_streams_.erase(stream_id);
    FX_DCHECK(!won_streams_awaiting_first_message_.contains(stream_id));
  }

  SendPendingIfWaiting();
}

void TouchSourceBase::EndContest(StreamId stream_id, bool awarded_win) {
  TRACE_DURATION("input", "TouchSourceBase::EndContest");

  inspector_.OnContestDecided(view_ref_koid_, awarded_win);

  auto it = ongoing_streams_.find(stream_id);
  if (it == ongoing_streams_.end()) {
    if (!awarded_win) {
      // Lost the stream before the first message.
      return;
    }
    const auto [_, success] = won_streams_awaiting_first_message_.emplace(stream_id);
    FX_DCHECK(success) << "Can't have two EndContest() calls for the same stream.";
    return;
  }

  auto& stream = it->second;
  FX_DCHECK(!stream.was_won) << "Can't have two EndContest() calls for the same stream.";
  stream.was_won = awarded_win;
  AugmentedTouchEvent event{
      .touch_event = NewEndEvent(stream_id, stream.device_id, stream.pointer_id, awarded_win)};
  PushEvent(stream_id, std::move(event));

  if (!awarded_win || stream.stream_has_ended) {
    ongoing_streams_.erase(stream_id);
  }

  SendPendingIfWaiting();
}

void TouchSourceBase::PushEvent(StreamId stream_id, AugmentedTouchEvent event) {
  pending_events_.push({.stream_id = stream_id, .event = std::move(event)});
}

zx_status_t TouchSourceBase::ValidateResponses(
    const std::vector<fuchsia_ui_pointer::TouchResponse>& responses,
    const std::vector<ReturnTicket>& return_tickets, bool have_pending_callback) {
  if (have_pending_callback) {
    FX_LOGS(ERROR) << "TouchSourceBase: Client called Watch twice without waiting for response.";
    return ZX_ERR_BAD_STATE;
  }

  if (return_tickets.size() != responses.size()) {
    FX_LOGS(ERROR)
        << "TouchSourceBase: Client called Watch with the wrong number of responses. Expected: "
        << return_tickets.size() << " Received: " << responses.size();
    return ZX_ERR_INVALID_ARGS;
  }

  for (size_t i = 0; i < responses.size(); ++i) {
    const auto& response = responses.at(i);
    if (!return_tickets.at(i).expects_response) {
      if (!response.IsEmpty()) {
        FX_LOGS(ERROR) << "TouchSourceBase: Expected empty response, receive non-empty response";
        return ZX_ERR_INVALID_ARGS;
      }
    } else {
      if (!response.response_type().has_value()) {
        FX_LOGS(ERROR) << "TouchSourceBase: Response was missing arguments.";
        return ZX_ERR_INVALID_ARGS;
      }

      if (ConvertToGestureResponse(response.response_type().value()) ==
          GestureResponse::kUndefined) {
        FX_LOGS(ERROR) << "TouchSourceBase: Response " << i << " had unknown response type.";
        return ZX_ERR_INVALID_ARGS;
      }
    }
  }

  return ZX_OK;
}

void TouchSourceBase::WatchBase(std::vector<fuchsia_ui_pointer::TouchResponse> responses,
                                fit::function<void(std::vector<AugmentedTouchEvent>)> callback) {
  TRACE_DURATION("input", "TouchSourceBase::Watch");
  const zx_status_t error = ValidateResponses(
      responses, return_tickets_, /*have_pending_callback*/ pending_callback_ != nullptr);
  if (error != ZX_OK) {
    CloseChannel(error);
    return;
  }

  // De-interlace responses from different streams.
  std::unordered_map<StreamId, std::vector<GestureResponse>> responses_per_stream;
  size_t index = 0;
  for (const auto& response : responses) {
    const auto [stream_id, expects_response] = return_tickets_.at(index++);
    if (!expects_response || !ongoing_streams_.contains(stream_id)) {
      continue;
    }

    const GestureResponse gd_response = ConvertToGestureResponse(response.response_type().value());
    responses_per_stream[stream_id].emplace_back(gd_response);

    auto& stream = ongoing_streams_[stream_id];
    stream.last_response = gd_response;
  }

  return_tickets_.clear();

  for (const auto& [stream_id, gd_responses] : responses_per_stream) {
    respond_(stream_id, gd_responses);
  }

  pending_callback_ = std::move(callback);
  SendPendingIfWaiting();
}

zx_status_t TouchSourceBase::ValidateUpdateResponse(
    const fuchsia_ui_pointer::TouchInteractionId& stream_identifier,
    const fuchsia_ui_pointer::TouchResponse& response,
    const std::unordered_map<StreamId, StreamData>& ongoing_streams) {
  const StreamId stream_id = stream_identifier.interaction_id();
  if (!ongoing_streams.contains(stream_id)) {
    FX_LOGS(ERROR)
        << "TouchSourceBase: Attempted to UpdateResponse for unkown stream. Received stream id: "
        << stream_id;
    return ZX_ERR_BAD_STATE;
  }

  if (!response.response_type().has_value()) {
    FX_LOGS(ERROR)
        << "TouchSourceBase: Can only UpdateResponse() called without response_type argument.";
    return ZX_ERR_INVALID_ARGS;
  }

  if (ConvertToGestureResponse(response.response_type().value()) == GestureResponse::kUndefined) {
    FX_LOGS(ERROR) << "TouchSourceBase: Response had unknown response type.";
    return ZX_ERR_INVALID_ARGS;
  }

  if (IsHold(response.response_type().value())) {
    FX_LOGS(ERROR) << "TouchSourceBase: Can only UpdateResponse() with non-HOLD response.";
    return ZX_ERR_INVALID_ARGS;
  }

  const auto& stream = ongoing_streams.at(stream_id);
  if (!IsHold(stream.last_response)) {
    FX_LOGS(ERROR) << "TouchSourceBase: Can only UpdateResponse() if previous response was HOLD.";
    return ZX_ERR_BAD_STATE;
  }

  if (!stream.stream_has_ended) {
    FX_LOGS(ERROR) << "TouchSourceBase: Can only UpdateResponse() for ended streams.";
    return ZX_ERR_BAD_STATE;
  }

  return ZX_OK;
}

void TouchSourceBase::UpdateResponseBase(
    const fuchsia_ui_pointer::TouchInteractionId& stream_identifier,
    fuchsia_ui_pointer::TouchResponse response, fit::function<void()> callback) {
  TRACE_DURATION("input", "TouchSourceBase::UpdateResponse");
  const zx_status_t error = ValidateUpdateResponse(stream_identifier, response, ongoing_streams_);
  if (error != ZX_OK) {
    CloseChannel(error);
    return;
  }

  if (response.trace_flow_id().has_value()) {
    TRACE_FLOW_END("input", "dispatch_event_to_client", response.trace_flow_id().value());
  }

  const StreamId stream_id = stream_identifier.interaction_id();
  const GestureResponse converted_response =
      ConvertToGestureResponse(response.response_type().value());
  ongoing_streams_.at(stream_id).last_response = converted_response;
  respond_(stream_id, {converted_response});

  callback();
}

void TouchSourceBase::SendPendingIfWaiting() {
  if (!pending_callback_ || pending_events_.empty()) {
    return;
  }
  FX_DCHECK(return_tickets_.empty());
  TRACE_DURATION("input", "TouchSourceBase::SendPendingIfWaiting");

  std::vector<AugmentedTouchEvent> events;
  for (size_t i = 0; !pending_events_.empty() && i < fuchsia_ui_pointer::kTouchMaxEvent; ++i) {
    auto [stream_id, event] = std::move(pending_events_.front());
    if (event.touch_event.trace_flow_id().has_value()) {
      TRACE_FLOW_STEP("input", "dispatch_event_to_client",
                      event.touch_event.trace_flow_id().value());
    }

    pending_events_.pop();
    return_tickets_.push_back({.stream_id = stream_id,
                               .expects_response = event.touch_event.pointer_sample().has_value()});
    events.emplace_back(std::move(event));
  }
  FX_DCHECK(!events.empty());
  FX_DCHECK(events.size() == return_tickets_.size());

  inspector_.OnInjectedEvents(view_ref_koid_, events.size());
  auto cb = std::move(pending_callback_);
  cb(std::move(events));
}

}  // namespace scenic_impl::input
