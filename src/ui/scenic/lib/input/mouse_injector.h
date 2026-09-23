// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_MOUSE_INJECTOR_H_
#define SRC_UI_SCENIC_LIB_INPUT_MOUSE_INJECTOR_H_

#include "src/ui/scenic/lib/input/injector.h"
#include "src/ui/scenic/lib/input/internal_pointer_event.h"

namespace scenic_impl::input {

// Implementation of the |fuchsia::ui::pointerinjector::Device| interface. One instance per channel.
class MouseInjector : public Injector {
 public:
  MouseInjector(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                inspect::Node inspect_node, InjectorSettings settings, Viewport viewport,
                fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
                fit::function<void(InternalMouseEvent, StreamId stream_id,
                                   const view_tree::Snapshot& snapshot)>
                    inject,
                fit::function<void(StreamId stream_id)> cancel_stream,
                fit::function<void()> on_channel_closed);

 protected:
  // |Injector|
  void ForwardEvent(fuchsia_ui_pointerinjector::wire::Event& event, StreamId stream_id,
                    const view_tree::Snapshot& snapshot, uint64_t trace_flow_id) override;
  // |Injector|
  void CancelStream(uint32_t pointer_id, StreamId stream_id,
                    const view_tree::Snapshot& snapshot) override;

 private:
  InternalMouseEvent PointerInjectorEventToInternalMouseEvent(
      fuchsia_ui_pointerinjector::wire::Event& event);

  // Used to inject the event into InputSystem for dispatch to clients.
  const fit::function<void(InternalMouseEvent, StreamId, const view_tree::Snapshot&)> inject_;
  // Explicit call necessary to cancel mouse stream, because mouse stream itself does not track
  // phase.
  const fit::function<void(StreamId)> cancel_stream_;
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_MOUSE_INJECTOR_H_
