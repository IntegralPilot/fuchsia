// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/input/input_system.h"

#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

#if defined(FUCHSIA_DSO)
#include <fidl/fuchsia.ui.pointerinjector.dso/cpp/driver/wire.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/token.h>
#endif

namespace scenic_impl::input {

InputSystem::InputSystem(async_dispatcher_t *input_dispatcher,
                         std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder,
                         inspect::Node &inspect_node, RequestFocusFunc request_focus,
                         sys::ComponentContext *context)
    : hit_tester_(inspect_node),
      mouse_system_(hit_tester_, std::move(request_focus)),
      touch_system_(input_dispatcher, hit_tester_, inspect_node),
#if !defined(FUCHSIA_DSO)
      pointerinjector_registry_(
          input_dispatcher, snapshot_holder,
          /*inject_touch_exclusive=*/
          [&touch_system = touch_system_](InternalTouchEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            touch_system.InjectTouchEventExclusive(std::move(event), stream_id, snapshot);
          },
          /*inject_touch_hit_tested=*/
          [&touch_system = touch_system_](InternalTouchEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            touch_system.InjectTouchEventHitTested(std::move(event), stream_id, snapshot);
          },
          /*inject_mouse_exclusive=*/
          [&mouse_system = mouse_system_](InternalMouseEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            mouse_system.InjectMouseEventExclusive(std::move(event), stream_id, snapshot);
          },
          /*inject_mouse_hit_tested=*/
          [&mouse_system = mouse_system_](InternalMouseEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            mouse_system.InjectMouseEventHitTested(std::move(event), stream_id, snapshot);
          },
          // Explicit call necessary to cancel mouse stream, because mouse stream itself does not
          // track phase.
          /*cancel_mouse_stream=*/
          [&mouse_system = mouse_system_](StreamId stream_id) {
            mouse_system.CancelMouseStream(stream_id);
          },
          inspect_node.CreateChild("PointerinjectorRegistry"))
#else
      pointerinjector_registry_(
          input_dispatcher, snapshot_holder,
          /*inject_touch_exclusive=*/
          [&touch_system = touch_system_](InternalTouchEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            touch_system.InjectTouchEventExclusive(std::move(event), stream_id, snapshot);
          },
          /*inject_touch_hit_tested=*/
          [&touch_system = touch_system_](InternalTouchEvent event, StreamId stream_id,
                                          const view_tree::Snapshot &snapshot) {
            touch_system.InjectTouchEventHitTested(std::move(event), stream_id, snapshot);
          },
          inspect_node.CreateChild("PointerinjectorRegistry"))
#endif
{
}

#if !defined(FUCHSIA_DSO)

void InputSystem::BindPointerinjectorRegistry(
    fidl::ServerEnd<fuchsia_ui_pointerinjector::Registry> server_end) {
  utils::CheckIsOnInputThread();
  pointerinjector_registry_.Bind(std::move(server_end));
}

#else

void InputSystem::BindPointerinjectorRegistry(zx::channel channel) {
  utils::CheckIsOnInputThread();
  zx_handle_t handle;
  zx_status_t s = fdf_token_receive(channel.release(), &handle);
  if (s != ZX_OK) {
    FX_LOGS(WARNING) << "FDF token failed cast to channel on "
                     << fuchsia_ui_pointerinjector::Registry::kDiscoverableName;
    return;
  }
  pointerinjector_registry_.Bind(fdf::Channel(handle));
}

#endif

void InputSystem::BindLocalHit(fidl::ServerEnd<fuchsia_ui_pointer_augment::LocalHit> server_end) {
  utils::CheckIsOnInputThread();
  touch_system_.Bind(std::move(server_end));
}

void InputSystem::BindA11yPointerEventRegistry(
    fidl::ServerEnd<fuchsia_ui_input_accessibility::PointerEventRegistry> request) {
  utils::CheckIsOnInputThread();
  touch_system_.BindA11yPointerEventRegistry(std::move(request));
}

void InputSystem::RegisterTouchSource(
    fidl::ServerEnd<fuchsia_ui_pointer::TouchSource> touch_source_server_end,
    zx_koid_t client_view_ref_koid) {
  utils::CheckIsOnInputThread();
  touch_system_.RegisterTouchSource(std::move(touch_source_server_end), client_view_ref_koid);
}

void InputSystem::RegisterTouchSourceV2(
    fidl::ServerEnd<fuchsia_ui_pointer::TouchSourceV2> touch_source_server_end,
    zx_koid_t client_view_ref_koid) {
  utils::CheckIsOnInputThread();
  touch_system_.RegisterTouchSourceV2(std::move(touch_source_server_end), client_view_ref_koid);
}

void InputSystem::RegisterMouseSource(
    fidl::ServerEnd<fuchsia_ui_pointer::MouseSource> mouse_source_server_end,
    zx_koid_t client_view_ref_koid) {
  utils::CheckIsOnInputThread();
  mouse_system_.RegisterMouseSource(std::move(mouse_source_server_end), client_view_ref_koid);
}

void InputSystem::RegisterMouseSourceV2(
    fidl::ServerEnd<fuchsia_ui_pointer::MouseSourceV2> mouse_source_server_end,
    zx_koid_t client_view_ref_koid) {
  utils::CheckIsOnInputThread();
  mouse_system_.RegisterMouseSourceV2(std::move(mouse_source_server_end), client_view_ref_koid);
}

}  // namespace scenic_impl::input
