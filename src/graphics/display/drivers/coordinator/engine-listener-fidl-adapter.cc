// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/engine-listener-fidl-adapter.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <memory>
#include <utility>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/drivers/coordinator/engine-listener.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

EngineListenerFidlAdapter::EngineListenerFidlAdapter(EngineListener* engine_listener,
                                                     fdf::UnownedSynchronizedDispatcher dispatcher)
    : engine_listener_(*engine_listener), dispatcher_(std::move(dispatcher)) {
  ZX_DEBUG_ASSERT(engine_listener != nullptr);
}

fidl::ProtocolHandler<fuchsia_hardware_display_engine::EngineListener>
EngineListenerFidlAdapter::CreateHandler() {
  return bindings_.CreateHandler(this, dispatcher_->get(), fidl::kIgnoreBindingClosure);
}

void EngineListenerFidlAdapter::OnDisplayAdded(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayAddedRequest* request,
    fdf::Arena& arena, OnDisplayAddedCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->get() == dispatcher_->get());
  zx::result<std::unique_ptr<AddedDisplayInfo>> added_display_info_result =
      AddedDisplayInfo::Create(request->display_info);
  if (added_display_info_result.is_error()) {
    // AddedDisplayInfo::Create() has already logged the error.
    return;
  }
  engine_listener_.OnDisplayAdded(std::move(added_display_info_result).value());
}

void EngineListenerFidlAdapter::OnDisplayRemoved(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayRemovedRequest* request,
    fdf::Arena& arena, OnDisplayRemovedCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->get() == dispatcher_->get());
  engine_listener_.OnDisplayRemoved(display::DisplayId(request->display_id.value));
}

void EngineListenerFidlAdapter::OnDisplayVsync(
    fuchsia_hardware_display_engine::wire::EngineListenerOnDisplayVsyncRequest* request,
    fdf::Arena& arena, OnDisplayVsyncCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->get() == dispatcher_->get());
  engine_listener_.OnDisplayVsync(display::DisplayId(request->display_id.value),
                                  zx::time_monotonic(request->timestamp),
                                  display::DriverConfigStamp(request->config_stamp.value));
}

void EngineListenerFidlAdapter::OnCaptureComplete(fdf::Arena& arena,
                                                  OnCaptureCompleteCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(fdf::Dispatcher::GetCurrent()->get() == dispatcher_->get());
  engine_listener_.OnCaptureComplete();
}

void EngineListenerFidlAdapter::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_display_engine::EngineListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("Unknown method ordinal: %lu", metadata.method_ordinal);
}

}  // namespace display_coordinator
