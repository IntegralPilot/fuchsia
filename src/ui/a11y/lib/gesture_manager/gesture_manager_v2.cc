// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/gesture_manager_v2.h"

#include <fuchsia/ui/input/accessibility/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdint>
#include <memory>
#include <optional>

#include "fuchsia/ui/pointer/cpp/fidl.h"
#include "src/ui/a11y/lib/gesture_manager/arena_v2/gesture_arena_v2.h"

namespace a11y {
namespace {

using fuchsia::ui::pointer::EventPhase;
using fuchsia::ui::pointer::Rectangle;
using fuchsia::ui::pointer::TouchResponse;
using fuchsia::ui::pointer::TouchResponseType;
using fuchsia::ui::pointer::augment::TouchEventWithLocalHit;
using fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr;

// Convert a `TouchInteractionId` into a triple of `uint32_t`, so that we can
// use it as the key in a `std::set`.
std::tuple<uint32_t, uint32_t, uint32_t> interactionToTriple(
    fuchsia::ui::pointer::TouchInteractionId interaction) {
  return {interaction.device_id, interaction.pointer_id, interaction.interaction_id};
}

// Helper for `normalizeToNdc`.
//
// Normalize `p` to be in the square [0, 1] * [0, 1].
//
// Returns std::nullopt if p is not contained in bounds.
std::optional<std::array<float, 2>> normalizeToUnitSquare(std::array<float, 2> p,
                                                          Rectangle bounds) {
  const float x = p[0];
  const float y = p[1];
  const float x_min = bounds.min[0];
  const float y_min = bounds.min[1];
  const float x_max = bounds.max[0];
  const float y_max = bounds.max[1];

  const bool in_bounds = (x_min <= x && x <= x_max) && (y_min <= y && y <= y_max);
  if (!in_bounds) {
    return std::nullopt;
  }

  const float width = x_max - x_min;
  const float height = y_max - y_min;
  const float dx = x - x_min;
  const float dy = y - y_min;

  return {{
      width > 0 ? dx / width : 0,
      height > 0 ? dy / height : 0,
  }};
}

// Normalize `p` to be in the square [-1, 1] * [-1, 1].
//
// Returns std::nullopt if p is not contained in bounds.
std::optional<std::array<float, 2>> normalizeToNdc(std::array<float, 2> p, Rectangle bounds) {
  auto normalized = normalizeToUnitSquare(p, bounds);
  if (!normalized) {
    return std::nullopt;
  }

  const float x = (*normalized)[0];
  const float y = (*normalized)[1];
  return {{
      x * 2 - 1,
      y * 2 - 1,
  }};
}

// Based on the status of the current a11y gesture arena contest, how should we
// respond in the system-level gesture disambiguation.
//
// Note that this is only the initial response; sometimes we'll have to say
// "hold" to indicate we don't know whether this interaction is ours yet. Once
// the current a11y gesture arena contest completes, we go back and update our
// responses.
TouchResponseType initialResponse(InteractionTracker::ConsumptionStatus status, EventPhase phase) {
  switch (status) {
    case InteractionTracker::ConsumptionStatus::kAccept:
      return TouchResponseType::YES_PRIORITIZE;
    case InteractionTracker::ConsumptionStatus::kReject:
      return TouchResponseType::NO;
    case InteractionTracker::ConsumptionStatus::kUndecided:
      switch (phase) {
        case EventPhase::ADD:
        case EventPhase::CHANGE:
          return TouchResponseType::MAYBE_PRIORITIZE_SUPPRESS;
        case EventPhase::REMOVE:
        case EventPhase::CANCEL:
          return TouchResponseType::HOLD_SUPPRESS;
      }
  }
}

// When a contest ends, any held interactions will have their responses updated.
//
// This simply translates from consumption status to response type.
TouchResponseType updatedResponse(InteractionTracker::ConsumptionStatus status) {
  switch (status) {
    case InteractionTracker::ConsumptionStatus::kUndecided:
      FX_DCHECK(false) << "held interactions should only be updated when the contest is resolved";
      return TouchResponseType::NO;
    case InteractionTracker::ConsumptionStatus::kAccept:
      return TouchResponseType::YES_PRIORITIZE;
    case InteractionTracker::ConsumptionStatus::kReject:
      return TouchResponseType::NO;
  }
}

}  // namespace

GestureManagerV2::GestureManagerV2(TouchSourceWithLocalHitPtr touch_source)
    : GestureManagerV2(std::move(touch_source),
                       [](InteractionTracker::HeldInteractionCallback callback) {
                         return std::make_unique<GestureArenaV2>(std::move(callback));
                       }) {}

GestureManagerV2::GestureManagerV2(TouchSourceWithLocalHitPtr touch_source,
                                   ArenaFactory arena_factory)
    : touch_source_(std::move(touch_source)),
      gesture_handler_([this](GestureRecognizerV2* recognizer) { AddRecognizer(recognizer); }) {
  // Park a callback that will notify the TouchSource (via UpdateResponse) when
  // a held interaction becomes decided.
  auto callback = [this](fuchsia::ui::pointer::TouchInteractionId interaction,
                         uint64_t trace_flow_id, InteractionTracker::ConsumptionStatus status) {
    FX_DCHECK(status != InteractionTracker::ConsumptionStatus::kUndecided);

    // Held interactions mean different things between Scenic and A11y:
    // A11y: a stream of pointer events started and ended, but no recognizer claimed them yet.
    // 2. Scenic: a11y submitted a "HOLD" response to an open interaction.
    //
    // It can be the case that A11y had a held interaction that gets resolved before we tell Scenic
    // to hold that interaction for us. This can happen because everything here is async: a11y,
    // Scenic, recognizers, input.
    //
    // In summary: only update a held interaction with Scenic if we previously told Scenic to hold
    // that one for us.
    auto it = held_interactions_.find(interactionToTriple(interaction));
    if (it != held_interactions_.end()) {
      TouchResponse response;
      response.set_response_type(updatedResponse(status));
      response.set_trace_flow_id(trace_flow_id);
      touch_source_->UpdateResponse(interaction, std::move(response), [](auto...) {});
      held_interactions_.erase(it);
    }
  };

  arena_ = arena_factory(std::move(callback));
  FX_DCHECK(arena_);

  WatchForTouchEvents({});
}

void GestureManagerV2::WatchForTouchEvents(std::vector<TouchResponse> old_responses) {
  auto callback = [this](std::vector<TouchEventWithLocalHit> events) {
    auto responses = HandleEvents(std::move(events));
    WatchForTouchEvents(std::move(responses));
  };
  touch_source_->Watch(std::move(old_responses), callback);
}

std::vector<fuchsia::ui::pointer::TouchResponse> GestureManagerV2::HandleEvents(
    std::vector<fuchsia::ui::pointer::augment::TouchEventWithLocalHit> events) {
  std::vector<TouchResponse> responses;

  for (uint32_t i = 0; i < events.size(); ++i) {
    auto& event = events[i];

    if (event.touch_event.has_view_parameters()) {
      viewport_bounds_ = event.touch_event.view_parameters().viewport;
    }

    // Warning: the field name `position_in_viewport` will no longer make sense.
    ConvertToNdc(event);

    auto response = HandleEvent(event);
    responses.push_back(std::move(response));
  }

  return responses;
}

fuchsia::ui::pointer::TouchResponse GestureManagerV2::HandleEvent(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  if (!event.touch_event.has_pointer_sample()) {
    // For non-sample events, the TouchSource API expects an empty response.
    return {};
  }

  fuchsia::ui::pointer::TouchResponse response;

  FX_DCHECK(event.touch_event.has_trace_flow_id());
  response.set_trace_flow_id(event.touch_event.trace_flow_id());

  FX_DCHECK(event.touch_event.pointer_sample().has_phase());
  const auto contest_status = arena_->OnEvent(event);
  const auto phase = event.touch_event.pointer_sample().phase();
  const auto response_type = initialResponse(contest_status, phase);
  if (response_type == TouchResponseType::HOLD_SUPPRESS) {
    auto interaction_id = interactionToTriple(event.touch_event.pointer_sample().interaction());
    held_interactions_.insert(interaction_id);
  }
  response.set_response_type(response_type);

  return response;
}

void GestureManagerV2::ConvertToNdc(fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  if (!event.touch_event.has_pointer_sample() ||
      !event.touch_event.pointer_sample().has_position_in_viewport()) {
    return;
  }
  FX_CHECK(viewport_bounds_) << "received touch sample event before viewport bounds";

  auto point = event.touch_event.mutable_pointer_sample()->mutable_position_in_viewport();
  *point = *normalizeToNdc(*point, *viewport_bounds_);
}

void GestureManagerV2::AddRecognizer(GestureRecognizerV2* recognizer) { arena_->Add(recognizer); }

void GestureManagerV2::Clear() {
  arena_->ClearRecognizers();
  gesture_handler_.Clear();
  held_interactions_.clear();
}

}  // namespace a11y
