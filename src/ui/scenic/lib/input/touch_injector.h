// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_TOUCH_INJECTOR_H_
#define SRC_UI_SCENIC_LIB_INPUT_TOUCH_INJECTOR_H_

#include "src/ui/scenic/lib/input/injector.h"

namespace scenic_impl::input {

// Implementation of the |fuchsia::ui::pointerinjector::Device| interface. One instance per channel.
// LINT.IfChange
class TouchInjector : public Injector {
 public:
  TouchInjector(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                inspect::Node inspect_node, InjectorSettings settings, Viewport viewport,
                fidl::ServerEnd<fuchsia_ui_pointerinjector::Device> device,
                fit::function<void(InternalTouchEvent, StreamId stream_id,
                                   const view_tree::Snapshot& snapshot)>
                    inject,
                fit::function<void()> on_channel_closed);

 protected:
  // |Injector|
  void ForwardEvent(fuchsia_ui_pointerinjector::wire::Event& event, StreamId stream_id,
                    const view_tree::Snapshot& snapshot, uint64_t trace_flow_id) override;
  // |Injector|
  void CancelStream(uint32_t pointer_id, StreamId stream_id,
                    const view_tree::Snapshot& snapshot) override;

 private:
  InternalTouchEvent PointerInjectorEventToInternalTouchEvent(
      fuchsia_ui_pointerinjector::wire::Event& event, uint64_t trace_flow_id);

  // Used to inject the event into InputSystem for dispatch to clients.
  const fit::function<void(InternalTouchEvent, StreamId, const view_tree::Snapshot&)> inject_;
};
// LINT.ThenChange(//src/ui/scenic/lib/input/dso/touch_injector.h)

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_TOUCH_INJECTOR_H_
