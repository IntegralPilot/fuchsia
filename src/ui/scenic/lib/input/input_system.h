// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_INPUT_INPUT_SYSTEM_H_
#define SRC_UI_SCENIC_LIB_INPUT_INPUT_SYSTEM_H_

#include <fidl/fuchsia.ui.pointer.augment/cpp/fidl.h>
#include <fidl/fuchsia.ui.pointer/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/channel.h>

#include <optional>

#include "src/ui/scenic/lib/input/mouse_system.h"
#include "src/ui/scenic/lib/input/pointerinjector_registry.h"
#include "src/ui/scenic/lib/input/touch_system.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"
#if defined(FUCHSIA_DSO)
#include "src/ui/scenic/lib/input/dso/pointerinjector_registry.h"  // nogncheck
#endif

namespace scenic_impl::input {

// Tracks and coordinates input APIs.
class InputSystem {
 public:
  InputSystem(async_dispatcher_t* input_dispatcher,
              std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
              inspect::Node& inspect_node, RequestFocusFunc request_focus,
              sys::ComponentContext* context = nullptr);
  ~InputSystem() = default;

#if !defined(FUCHSIA_DSO)
  void BindPointerinjectorRegistry(
      fidl::ServerEnd<fuchsia_ui_pointerinjector::Registry> server_end);
#else
  void BindPointerinjectorRegistry(zx::channel channel);
#endif
  void BindLocalHit(fidl::ServerEnd<fuchsia_ui_pointer_augment::LocalHit> server_end);
  void BindA11yPointerEventRegistry(
      fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request);

  // Delegates to `touch_system_`.
  void RegisterTouchSource(fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source_server_end,
                           zx_koid_t client_view_ref_koid);

  void RegisterTouchSourceV2(
      fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source_server_end,
      zx_koid_t client_view_ref_koid);

  // Delegates to `mouse_system_`.
  void RegisterMouseSource(fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source_server_end,
                           zx_koid_t client_view_ref_koid);

  void RegisterMouseSourceV2(
      fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source_server_end,
      zx_koid_t client_view_ref_koid);

 private:
  HitTester hit_tester_;
  MouseSystem mouse_system_;
  TouchSystem touch_system_;
#if defined(FUCHSIA_DSO)
  ::scenic_impl::input_dso::PointerinjectorRegistry pointerinjector_registry_;
#else
  PointerinjectorRegistry pointerinjector_registry_;
#endif
};

}  // namespace scenic_impl::input

#endif  // SRC_UI_SCENIC_LIB_INPUT_INPUT_SYSTEM_H_
