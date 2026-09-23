// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_DSO_POINTERINJECTOR_REGISTRY_H_
#define SRC_UI_SCENIC_LIB_INPUT_DSO_POINTERINJECTOR_REGISTRY_H_

#include <fidl/fuchsia.ui.pointerinjector.dso/cpp/driver/wire.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sys/cpp/component_context.h>

#include <unordered_map>

#include "src/ui/scenic/lib/input/dso/injector.h"
#include "src/ui/scenic/lib/input/dso/touch_injector.h"
#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

namespace scenic_impl::input_dso {

using TouchInjectFunc = fit::function<void(InternalTouchEvent event, StreamId stream_id,
                                           const view_tree::Snapshot& snapshot)>;

// Handles the registration and config validation of fuchsia::ui::pointerinjector_dso clients.
// LINT.IfChange
class PointerinjectorRegistry : public fdf::WireServer<fuchsia_ui_pointerinjector_dso::Registry> {
 public:
  PointerinjectorRegistry(async_dispatcher_t* input_dispatcher,
                          std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                          TouchInjectFunc inject_touch_exclusive,
                          TouchInjectFunc inject_touch_hit_tested,
                          inspect::Node inspect_node = inspect::Node());

  void Bind(fdf::Channel channel);

  void Register(RegisterRequestView Request, fdf::Arena& arena,
                RegisterCompleter::Sync& completer) override;

 private:
  using InjectorId = uint64_t;
  InjectorId last_injector_id_ = 0;
  std::unordered_map<InjectorId, std::unique_ptr<Injector>> injectors_;

  fdf::ServerBindingGroup<fuchsia_ui_pointerinjector_dso::Registry> injector_registry_;

  const TouchInjectFunc inject_touch_exclusive_;
  const TouchInjectFunc inject_touch_hit_tested_;

  const std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder_;

  async_dispatcher_t* const input_dispatcher_;
  inspect::Node inspect_node_;
};
// LINT.ThenChange(//src/ui/scenic/lib/input/pointerinjector_registry.h)

}  // namespace scenic_impl::input_dso

#endif  // SRC_UI_SCENIC_LIB_INPUT_DSO_POINTERINJECTOR_REGISTRY_H_
