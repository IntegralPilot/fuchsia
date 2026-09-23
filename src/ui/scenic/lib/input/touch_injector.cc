// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/touch_injector.h"

#include <fidl/fuchsia.ui.pointerinjector/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

namespace scenic_impl::input {

using InjectorEventPhase = fuchsia_ui_pointerinjector::wire::EventPhase;

namespace {

InternalTouchEvent CreateCancelEvent(uint32_t device_id, uint32_t pointer_id, zx_koid_t context,
                                     zx_koid_t target) {
  InternalTouchEvent cancel_event;
  cancel_event.phase = Phase::kCancel;
  cancel_event.device_id = device_id;
  cancel_event.pointer_id = pointer_id;
  cancel_event.context = context;
  cancel_event.target = target;
  return cancel_event;
}

}  // namespace

// LINT.IfChange
TouchInjector::TouchInjector(
    std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder, inspect::Node inspect_node,
    InjectorSettings settings, Viewport viewport,
    fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
    fit::function<void(InternalTouchEvent, StreamId stream_id, const view_tree::Snapshot& snapshot)>
        inject,
    fit::function<void()> on_channel_closed)
    : Injector(std::move(snapshot_holder), std::move(inspect_node), std::move(settings),
               std::move(viewport), std::move(device), std::move(on_channel_closed)),
      inject_(std::move(inject)) {
  FX_DCHECK(inject_);
  FX_DCHECK(settings.device_type == fuchsia_ui_pointerinjector::wire::DeviceType::kTouch);
}

void TouchInjector::ForwardEvent(fuchsia_ui_pointerinjector::wire::Event& event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot, uint64_t trace_flow_id) {
  TRACE_DURATION("input", "TouchInjector::ForwardEvent");
  FX_DCHECK(stream_id != kInvalidStreamId);
  inject_(PointerInjectorEventToInternalTouchEvent(event, trace_flow_id), stream_id, snapshot);
}

InternalTouchEvent TouchInjector::PointerInjectorEventToInternalTouchEvent(
    fuchsia_ui_pointerinjector::wire::Event& event, uint64_t trace_flow_id) {
  const InjectorSettings& settings = Injector::settings();
  InternalTouchEvent internal_event;
  if (event.has_wake_lease()) {
    internal_event.wake_lease = std::move(event.wake_lease());
  }
  internal_event.timestamp = event.timestamp();
  internal_event.device_id = settings.device_id;
  internal_event.trace_flow_id = trace_flow_id;

  const fuchsia_ui_pointerinjector::wire::PointerSample& pointer_sample =
      event.data().pointer_sample();
  internal_event.pointer_id = pointer_sample.pointer_id();
  internal_event.viewport = viewport();
  internal_event.position_in_viewport = {pointer_sample.position_in_viewport()[0],
                                         pointer_sample.position_in_viewport()[1]};
  internal_event.context = settings.context_koid;
  internal_event.target = settings.target_koid;

  switch (pointer_sample.phase()) {
    case InjectorEventPhase::kAdd: {
      internal_event.phase = Phase::kAdd;
      break;
    }
    case InjectorEventPhase::kChange: {
      internal_event.phase = Phase::kChange;
      break;
    }
    case InjectorEventPhase::kRemove: {
      internal_event.phase = Phase::kRemove;
      break;
    }
    case InjectorEventPhase::kCancel: {
      internal_event.phase = Phase::kCancel;
      break;
    }
    default: {
      FX_CHECK(false) << "unsupported phase: " << static_cast<uint32_t>(pointer_sample.phase());
      break;
    }
  }

  return internal_event;
}

void TouchInjector::CancelStream(uint32_t pointer_id, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot) {
  const InjectorSettings& settings = Injector::settings();
  inject_(CreateCancelEvent(settings.device_id, pointer_id, settings.context_koid,
                            settings.target_koid),
          stream_id, snapshot);
}
// LINT.ThenChange(//src/ui/scenic/lib/input/dso/touch_injector.cc)

}  // namespace scenic_impl::input
