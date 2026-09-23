// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/helper.h"

#include <lib/trace/event.h>

#include "src/ui/scenic/lib/utils/math.h"

namespace scenic_impl::input {

using PointerEventPhase = fuchsia_ui_input::PointerEventPhase;
using GfxPointerEvent = fuchsia_ui_input::PointerEvent;

std::pair<float, float> ReversePointerTraceHACK(trace_flow_id_t trace_id) {
  float fhigh, flow;
  const uint32_t ihigh = (uint32_t)(trace_id >> 32);
  const uint32_t ilow = (uint32_t)trace_id;
  memcpy(&fhigh, &ihigh, sizeof(uint32_t));
  memcpy(&flow, &ilow, sizeof(uint32_t));
  return {fhigh, flow};
}

PointerEventPhase InternalPhaseToGfxPhase(Phase phase) {
  switch (phase) {
    case Phase::kAdd:
      return PointerEventPhase::kAdd;
    case Phase::kChange:
      return PointerEventPhase::kMove;
    case Phase::kRemove:
      return PointerEventPhase::kRemove;
    case Phase::kCancel:
      return PointerEventPhase::kCancel;
    case Phase::kInvalid:
      FX_CHECK(false) << "Should never be reached.";
      return static_cast<PointerEventPhase>(0);
  };
}

}  // namespace scenic_impl::input
