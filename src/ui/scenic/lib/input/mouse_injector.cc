// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/mouse_injector.h"

#include <fidl/fuchsia.ui.pointerinjector/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

namespace scenic_impl::input {

namespace {

ScrollInfo CreateScrollInfo(const fuchsia_input::wire::Axis& axis,
                            std::optional<int64_t> scroll_value) {
  ScrollInfo scroll_info = {
      .unit = axis.unit.type,
      .exponent = axis.unit.exponent,
      .range = {axis.range.min, axis.range.max},
  };

  if (scroll_value.has_value()) {
    scroll_info.scroll_value = scroll_value.value();
  }

  return scroll_info;
}

}  // namespace

MouseInjector::MouseInjector(
    std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder, inspect::Node inspect_node,
    InjectorSettings settings, Viewport viewport,
    fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
    fit::function<void(InternalMouseEvent, StreamId stream_id, const view_tree::Snapshot& snapshot)>
        inject,
    fit::function<void(StreamId stream_id)> cancel_stream, fit::function<void()> on_channel_closed)
    : Injector(std::move(snapshot_holder), std::move(inspect_node), std::move(settings),
               std::move(viewport), std::move(device), std::move(on_channel_closed)),
      inject_(std::move(inject)),
      cancel_stream_(std::move(cancel_stream)) {
  FX_DCHECK(inject_);
  FX_DCHECK(settings.device_type == fuchsia_ui_pointerinjector::wire::DeviceType::kMouse);
}

void MouseInjector::ForwardEvent(fuchsia_ui_pointerinjector::wire::Event& event, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot, uint64_t trace_flow_id) {
  TRACE_DURATION("input", "MouseInjector::ForwardEvent");
  {  // For CANCEL and REMOVE phase we need to cancel the stream. Otherwise inject normally.
    FX_DCHECK(event.has_data());
    const auto& data = event.data();
    if (data.is_pointer_sample()) {
      FX_DCHECK(data.pointer_sample().has_phase());
      const auto phase = data.pointer_sample().phase();
      if (phase == fuchsia_ui_pointerinjector::wire::EventPhase::kCancel ||
          phase == fuchsia_ui_pointerinjector::wire::EventPhase::kRemove) {
        cancel_stream_(stream_id);
        return;
      }
    }
  }

  inject_(PointerInjectorEventToInternalMouseEvent(event), stream_id, snapshot);
}

InternalMouseEvent MouseInjector::PointerInjectorEventToInternalMouseEvent(
    fuchsia_ui_pointerinjector::wire::Event& event) {
  FX_DCHECK(event.has_data());
  FX_DCHECK(event.data().is_pointer_sample());

  InternalMouseEvent internal_event;
  const InjectorSettings& settings = Injector::settings();

  if (event.has_wake_lease()) {
    internal_event.wake_lease = std::move(event.wake_lease());
  }

  // General
  internal_event.timestamp = event.timestamp();
  internal_event.device_id = settings.device_id;
  internal_event.context = settings.context_koid;
  internal_event.target = settings.target_koid;

  const fuchsia_ui_pointerinjector::wire::PointerSample& pointer_sample =
      event.data().pointer_sample();
  // Coordinates
  internal_event.viewport = viewport();
  internal_event.position_in_viewport = {pointer_sample.position_in_viewport()[0],
                                         pointer_sample.position_in_viewport()[1]};

  // Buttons
  internal_event.buttons = {.identifiers = settings.button_identifiers};
  if (pointer_sample.has_pressed_buttons()) {
    const auto& pressed = pointer_sample.pressed_buttons();
    internal_event.buttons.pressed = std::vector<uint8_t>(pressed.begin(), pressed.end());
  }

  // Scroll V
  if (settings.scroll_v_range.has_value()) {
    std::optional<int64_t> scroll_value;
    if (pointer_sample.has_scroll_v()) {
      scroll_value = pointer_sample.scroll_v();
    }
    internal_event.scroll_v = CreateScrollInfo(settings.scroll_v_range.value(), scroll_value);
  }

  // Scroll H
  if (settings.scroll_h_range.has_value()) {
    std::optional<int64_t> scroll_value;
    if (pointer_sample.has_scroll_h()) {
      scroll_value = pointer_sample.scroll_h();
    }
    internal_event.scroll_h = CreateScrollInfo(settings.scroll_h_range.value(), scroll_value);
  }

  if (pointer_sample.has_scroll_v_physical_pixel()) {
    internal_event.scroll_v_physical_pixel = pointer_sample.scroll_v_physical_pixel();
  }

  if (pointer_sample.has_scroll_h_physical_pixel()) {
    internal_event.scroll_h_physical_pixel = pointer_sample.scroll_h_physical_pixel();
  }

  if (pointer_sample.has_is_precision_scroll()) {
    internal_event.is_precision_scroll = pointer_sample.is_precision_scroll();
  }

  // Relative Motion
  if (pointer_sample.has_relative_motion()) {
    internal_event.relative_motion = {pointer_sample.relative_motion()[0],
                                      pointer_sample.relative_motion()[1]};
  }

  return internal_event;
}

void MouseInjector::CancelStream(uint32_t pointer_id, StreamId stream_id,
                                 const view_tree::Snapshot& snapshot) {
  cancel_stream_(stream_id);
}

}  // namespace scenic_impl::input
