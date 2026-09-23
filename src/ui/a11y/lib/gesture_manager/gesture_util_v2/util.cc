// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/gesture_manager/gesture_util_v2/util.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>

namespace a11y::gesture_util_v2 {

namespace {

// Trivial conversion, just massaging types.
::fuchsia::math::PointF Point2ToPointF(std::array<float, 2> point) { return {point[0], point[1]}; }

::fuchsia::math::PointF Centroid(const std::vector<::fuchsia::math::PointF>& points) {
  ::fuchsia::math::PointF centroid;

  for (const auto& point : points) {
    centroid.x += point.x;
    centroid.y += point.y;
  }

  centroid.x /= static_cast<float>(points.size());
  centroid.y /= static_cast<float>(points.size());

  return centroid;
}

void UpdateLastEventInfo(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event,
                         GestureContext* gesture_context) {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  gesture_context->last_event_pointer_id = sample.interaction().pointer_id;

  FX_DCHECK(event.touch_event.has_timestamp());
  gesture_context->last_event_time = event.touch_event.timestamp();

  FX_DCHECK(sample.has_phase());
  gesture_context->last_event_phase = sample.phase();
}

}  // namespace

::fuchsia::math::PointF GestureContext::StartingCentroid(bool local) const {
  std::vector<::fuchsia::math::PointF> points;
  for (const auto& it : starting_pointer_locations) {
    points.push_back(local ? it.second.local_point : it.second.ndc_point);
  }

  return Centroid(points);
}

::fuchsia::math::PointF GestureContext::CurrentCentroid(bool local) const {
  std::vector<::fuchsia::math::PointF> points;
  for (const auto& it : current_pointer_locations) {
    points.push_back(local ? it.second.local_point : it.second.ndc_point);
  }

  return Centroid(points);
}

void InitializeStartingGestureContext(
    const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event,
    GestureContext* gesture_context) {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  uint32_t pointer_id = sample.interaction().pointer_id;

  gesture_context->view_ref_koid = event.local_viewref_koid;

  FX_DCHECK(sample.has_position_in_viewport());
  PointerLocation location = {
      .pointer_on_screen = true,
      .ndc_point = Point2ToPointF(sample.position_in_viewport()),
      .local_point = Point2ToPointF(event.local_point),
  };

  gesture_context->starting_pointer_locations[pointer_id] =
      gesture_context->current_pointer_locations[pointer_id] = location;

  UpdateLastEventInfo(event, gesture_context);
}

void UpdateGestureContext(const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event,
                          bool pointer_on_screen, GestureContext* gesture_context) {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  uint32_t pointer_id = sample.interaction().pointer_id;

  gesture_context->view_ref_koid = event.local_viewref_koid;

  FX_DCHECK(sample.has_position_in_viewport());
  gesture_context->current_pointer_locations[pointer_id].ndc_point =
      Point2ToPointF(sample.position_in_viewport());

  gesture_context->current_pointer_locations[pointer_id].local_point =
      Point2ToPointF(event.local_point);

  gesture_context->current_pointer_locations[pointer_id].pointer_on_screen = pointer_on_screen;

  UpdateLastEventInfo(event, gesture_context);
}

uint32_t NumberOfFingersOnScreen(const GestureContext& gesture_context) {
  uint32_t num_fingers = 0;
  for (const auto& it : gesture_context.current_pointer_locations) {
    if (it.second.pointer_on_screen) {
      num_fingers++;
    }
  }

  return num_fingers;
}

bool FingerIsOnScreen(const GestureContext& gesture_context, uint32_t pointer_id) {
  if (!gesture_context.current_pointer_locations.count(pointer_id)) {
    return false;
  }

  return gesture_context.current_pointer_locations.at(pointer_id).pointer_on_screen;
}

void ResetGestureContext(GestureContext* gesture_context) {
  gesture_context->view_ref_koid = ZX_KOID_INVALID;
  gesture_context->starting_pointer_locations.clear();
  gesture_context->current_pointer_locations.clear();
}

bool ValidateTouchEvent(const GestureContext& gesture_context,
                        const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  // Check if `event` has all the required fields.
  if (event.touch_event.has_timestamp() && event.touch_event.has_pointer_sample() &&
      event.touch_event.pointer_sample().has_interaction()) {
    uint32_t pointer_id = event.touch_event.pointer_sample().interaction().pointer_id;
    return gesture_context.starting_pointer_locations.count(pointer_id);
  }

  FX_LOGS(INFO) << "Touch event is missing required information.";
  return false;
}

bool TouchEventIsValidTap(const GestureContext& gesture_context,
                          const fuchsia::ui::pointer::augment::TouchEventWithLocalHit& event) {
  FX_DCHECK(event.touch_event.has_pointer_sample());
  const auto& sample = event.touch_event.pointer_sample();

  FX_DCHECK(sample.has_interaction());
  uint32_t pointer_id = sample.interaction().pointer_id;

  if (!gesture_context.starting_pointer_locations.count(pointer_id)) {
    return false;
  }

  FX_DCHECK(sample.has_position_in_viewport());
  return SquareDistanceBetweenPoints(
             Point2ToPointF(sample.position_in_viewport()),
             gesture_context.starting_pointer_locations.at(pointer_id).ndc_point) <=
         kGestureMoveThreshold * kGestureMoveThreshold;
}

float SquareDistanceBetweenPoints(::fuchsia::math::PointF a, ::fuchsia::math::PointF b) {
  auto dx = a.x - b.x;
  auto dy = a.y - b.y;

  return dx * dx + dy * dy;
}

}  // namespace a11y::gesture_util_v2
