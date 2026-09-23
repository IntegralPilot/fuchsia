// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_FIDL_ADAPTER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_FIDL_ADAPTER_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/fdf/cpp/dispatcher.h>

#include "src/graphics/display/drivers/coordinator/engine-listener.h"

namespace display_coordinator {

// Translates FIDL API calls to `EngineListener` C++ method calls.
//
// This adapter implements the
// [`fuchsia.hardware.display.engine/EngineListener`] FIDL API.
class EngineListenerFidlAdapter
    : public fdf::WireServer<fuchsia_hardware_display_engine::EngineListener> {
 public:
  // `engine_listener` must be valid and outlive `dispatcher`.
  // `engine_listener` is only invoked on `dispatcher`.
  explicit EngineListenerFidlAdapter(EngineListener* engine_listener,
                                     fdf::UnownedSynchronizedDispatcher dispatcher);

  fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener> CreateHandler();

  // `fdf::WireServer<fuchsia_hardware_display_engine::EngineListener>`:
  // Must be invoked on `dispatcher_`.
  void OnDisplayAdded(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
      fdf::Arena& arena, OnDisplayAddedCompleter::Sync& completer) override;
  void OnDisplayRemoved(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
      fdf::Arena& arena, OnDisplayRemovedCompleter::Sync& completer) override;
  void OnDisplayVsync(
      fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
      fdf::Arena& arena, OnDisplayVsyncCompleter::Sync& completer) override;
  void OnCaptureComplete(fdf::Arena& arena, OnCaptureCompleteCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display_engine::EngineListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  EngineListener& engine_listener_;
  fdf::UnownedSynchronizedDispatcher dispatcher_;
  fdf::ServerBindingGroup<fuchsia_hardware_display_engine::EngineListener> bindings_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ENGINE_LISTENER_FIDL_ADAPTER_H_
