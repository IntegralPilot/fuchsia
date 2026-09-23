// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_POINTERINJECTOR_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_INPUT_POINTERINJECTOR_REGISTRY_H_

#include <fidl/fuchsia.ui.pointerinjector/cpp/wire.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sys/cpp/component_context.h>

#include <unordered_map>

#include "src/ui/scenic/lib/input/injector.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

namespace scenic_impl::input {

using TouchInjectFunc = fit::function<void(InternalTouchEvent event, StreamId stream_id,
                                           const view_tree::Snapshot& snapshot)>;
using MouseInjectFunc = fit::function<void(InternalMouseEvent event, StreamId stream_id,
                                           const view_tree::Snapshot& snapshot)>;

// Handles the registration and config validation of fuchsia::ui::pointerinjector clients.
// LINT.IfChange
class PointerinjectorRegistry : public fidl::WireServer<fuchsia_ui_pointerinjector::Registry> {
 public:
  PointerinjectorRegistry(async_dispatcher_t* input_dispatcher,
                          std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                          TouchInjectFunc inject_touch_exclusive,
                          TouchInjectFunc inject_touch_hit_tested,
                          MouseInjectFunc inject_mouse_exclusive,
                          MouseInjectFunc inject_mouse_hit_tested,
                          fit::function<void(StreamId stream_id)> cancel_mouse_stream,
                          inspect::Node inspect_node = inspect::Node());

  void Bind(fidl::ServerEnd<fuchsia_ui_pointerinjector::Registry> server_end);

  // |fidl::WireServer<fuchsia_ui_pointerinjector::Registry>|
  void Register(RegisterRequestView request, RegisterCompleter::Sync& completer) override;

 private:
  using InjectorId = uint64_t;
  InjectorId last_injector_id_ = 0;
  std::unordered_map<InjectorId, std::unique_ptr<Injector>> injectors_;

  fidl::ServerBindingGroup<fuchsia_ui_pointerinjector::Registry> injector_registry_;

  const TouchInjectFunc inject_touch_exclusive_;
  const TouchInjectFunc inject_touch_hit_tested_;
  const MouseInjectFunc inject_mouse_exclusive_;
  const MouseInjectFunc inject_mouse_hit_tested_;
  const fit::function<void(StreamId stream_id)> cancel_mouse_stream_;

  const std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;

  async_dispatcher_t* const input_dispatcher_;
  inspect::Node inspect_node_;
};
// LINT.ThenChange(//src/ui/scenic/lib/input/dso/pointerinjector_registry.h)

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_POINTERINJECTOR_REGISTRY_H_
