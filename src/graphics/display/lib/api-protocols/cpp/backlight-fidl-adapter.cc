// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/backlight-fidl-adapter.h"

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"

namespace display {

BacklightFidlAdapter::BacklightFidlAdapter(BacklightInterface* backlight) : backlight_(*backlight) {
  ZX_DEBUG_ASSERT(backlight != nullptr);
}

BacklightFidlAdapter::~BacklightFidlAdapter() = default;

fidl::ProtocolHandler<fuchsia_hardware_backlight::Device> BacklightFidlAdapter::CreateHandler(
    async_dispatcher_t& dispatcher) {
  return bindings_.CreateHandler(this, &dispatcher, fidl::kIgnoreBindingClosure);
}

void BacklightFidlAdapter::GetStateNormalized(GetStateNormalizedCompleter::Sync& completer) {
  const zx::result<BacklightState> state_result = backlight_.GetBacklightState();
  if (state_result.is_error()) {
    completer.ReplyError(state_result.error_value());
    return;
  }
  completer.ReplySuccess(state_result->ToFidl(/*use_absolute_brightness=*/false));
}

void BacklightFidlAdapter::SetStateNormalized(
    fuchsia_hardware_backlight::wire::DeviceSetStateNormalizedRequest* request,
    SetStateNormalizedCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(request != nullptr);
  if (!BacklightState::IsValid(request->state, /*use_absolute_brightness=*/false)) {
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  const zx::result<> set_state_result = backlight_.SetBacklightState(
      BacklightState::From(request->state, /*use_absolute_brightness=*/false));
  completer.Reply(set_state_result);
}

void BacklightFidlAdapter::GetStateAbsolute(GetStateAbsoluteCompleter::Sync& completer) {
  const zx::result<BacklightState> state_result = backlight_.GetBacklightState();
  if (state_result.is_error()) {
    completer.ReplyError(state_result.error_value());
    return;
  }
  if (!state_result->brightness_nits().has_value()) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  completer.ReplySuccess(state_result->ToFidl(/*use_absolute_brightness=*/true));
}

void BacklightFidlAdapter::SetStateAbsolute(
    fuchsia_hardware_backlight::wire::DeviceSetStateAbsoluteRequest* request,
    SetStateAbsoluteCompleter::Sync& completer) {
  ZX_DEBUG_ASSERT(request != nullptr);
  if (!BacklightState::IsValid(request->state, /*use_absolute_brightness=*/true)) {
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  const zx::result<> set_state_result = backlight_.SetBacklightState(
      BacklightState::From(request->state, /*use_absolute_brightness=*/true));
  completer.Reply(set_state_result);
}

void BacklightFidlAdapter::GetMaxAbsoluteBrightness(
    GetMaxAbsoluteBrightnessCompleter::Sync& completer) {
  const zx::result<float> nits_result = backlight_.GetMaxBrightnessNits();
  if (nits_result.is_error()) {
    completer.ReplyError(nits_result.error_value());
    return;
  }
  completer.ReplySuccess(nits_result.value());
}

}  // namespace display
