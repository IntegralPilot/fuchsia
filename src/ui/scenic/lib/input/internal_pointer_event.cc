// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/internal_pointer_event.h"

namespace scenic_impl::input {

InternalTouchEvent InternalTouchEvent::ShallowClone() const {
  InternalTouchEvent clone;
  clone.timestamp = timestamp;
  clone.device_id = device_id;
  clone.pointer_id = pointer_id;
  clone.phase = phase;
  clone.context = context;
  clone.target = target;
  clone.viewport = viewport;
  clone.position_in_viewport = position_in_viewport;
  clone.buttons = buttons;
  clone.trace_flow_id = trace_flow_id;
  return clone;
}

InternalMouseEvent InternalMouseEvent::ShallowClone() const {
  InternalMouseEvent clone;
  clone.timestamp = timestamp;
  clone.device_id = device_id;
  clone.context = context;
  clone.target = target;
  clone.viewport = viewport;
  clone.position_in_viewport = position_in_viewport;
  clone.buttons = buttons;
  clone.scroll_v = scroll_v;
  clone.scroll_h = scroll_h;
  clone.scroll_v_physical_pixel = scroll_v_physical_pixel;
  clone.scroll_h_physical_pixel = scroll_h_physical_pixel;
  clone.is_precision_scroll = is_precision_scroll;
  clone.relative_motion = relative_motion;
  return clone;
}

}  // namespace scenic_impl::input
