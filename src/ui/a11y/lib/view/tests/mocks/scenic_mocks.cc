// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/view/tests/mocks/scenic_mocks.h"

namespace accessibility_test {
namespace {

using fuchsia::ui::pointer::EventPhase;
using fuchsia::ui::pointer::TouchEvent;
using fuchsia::ui::pointer::TouchInteractionId;
using fuchsia::ui::pointer::TouchPointerSample;
using fuchsia::ui::pointer::TouchResponse;
using fuchsia::ui::pointer::TouchResponseType;
using fuchsia::ui::pointer::ViewParameters;
using fuchsia::ui::pointer::augment::TouchEventWithLocalHit;
using fuchsia::ui::pointer::augment::TouchSourceWithLocalHit;
using fuchsia::ui::pointer::augment::TouchSourceWithLocalHitPtr;

TouchEventWithLocalHit fake_touch_event(EventPhase phase, uint64_t view_ref_koid_for_hit = 0,
                                        uint32_t interaction_id = 0,
                                        std::array<float, 2> position_in_viewport = {0, 0}) {
  TouchPointerSample sample;
  sample.set_interaction({0, 0, interaction_id});
  sample.set_phase(phase);
  sample.set_position_in_viewport(position_in_viewport);

  TouchEvent inner;
  inner.set_timestamp(0);
  inner.set_pointer_sample(std::move(sample));
  inner.set_trace_flow_id(0);

  return {std::move(inner), view_ref_koid_for_hit, {0, 0}};
}

TouchEventWithLocalHit fake_view_parameters() {
  const ViewParameters parameters = {
      .view = {{0, 0}, {1, 1}},
      .viewport = {{0, 0}, {1, 1}},
      .viewport_to_view_transform = {0},
  };

  TouchEvent inner;
  inner.set_view_parameters(parameters);

  return {
      .touch_event = std::move(inner),
      .local_viewref_koid = 0,
      .local_point = {0, 0},
  };
}

/*
bool interaction_equals(TouchInteractionId id1, TouchInteractionId id2) {
  return id1.device_id == id2.device_id && id1.pointer_id == id2.pointer_id &&
         id1.interaction_id == id2.interaction_id;
}
*/

}  // namespace

MockLocalHit::MockLocalHit() : touch_source_binding_(this) {}

void MockLocalHit::Watch(std::vector<TouchResponse> responses, WatchCallback callback) {
  ++num_watch_calls_;
  responses_ = std::move(responses);
  callback_ = std::move(callback);
}

void MockLocalHit::UpdateResponse(TouchInteractionId interaction, TouchResponse response,
                                  UpdateResponseCallback callback) {
  updated_responses_.emplace_back(std::make_pair(interaction, std::move(response)));
}

uint32_t MockLocalHit::NumWatchCalls() const { return num_watch_calls_; }

void MockLocalHit::SimulateEvents(std::vector<TouchEventWithLocalHit> events) {
  FX_CHECK(callback_);
  callback_(std::move(events));
  callback_ = nullptr;
}

std::vector<TouchResponse> MockLocalHit::TakeResponses() { return std::move(responses_); }

std::vector<std::pair<TouchInteractionId, TouchResponse>> MockLocalHit::TakeUpdatedResponses() {
  return std::move(updated_responses_);
}

void MockLocalHit::EnqueueTapToEvents() {
  if (!view_parameters_sent_) {
    enqueued_events_.emplace_back(fake_view_parameters());
    view_parameters_sent_ = true;
  }
  enqueued_events_.emplace_back(fake_touch_event(EventPhase::ADD, view_ref_koid_for_hit_));
  enqueued_events_.emplace_back(fake_touch_event(EventPhase::CHANGE, view_ref_koid_for_hit_));
  enqueued_events_.emplace_back(fake_touch_event(EventPhase::REMOVE, view_ref_koid_for_hit_));
}

void MockLocalHit::SimulateEnqueuedEvents() {
  SimulateEvents(std::move(enqueued_events_));
  enqueued_events_.clear();
}

void MockLocalHit::SetViewRefKoidForTouchEvents(uint64_t view_ref_koid) {
  view_ref_koid_for_hit_ = view_ref_koid;
}

}  // namespace accessibility_test
