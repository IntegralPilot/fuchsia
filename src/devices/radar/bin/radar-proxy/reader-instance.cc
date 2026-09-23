// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reader-instance.h"

#include <lib/syslog/cpp/macros.h>

namespace radar {

void ReaderInstance::GetBurstProperties(GetBurstPropertiesCompleter::Sync& completer) {
  completer.Reply(parent_->burst_properties());
}

void ReaderInstance::RegisterVmos(RegisterVmosRequest& request,
                                  RegisterVmosCompleter::Sync& completer) {
  const fit::result result = manager_.RegisterVmos(request.vmo_ids(), std::move(request.vmos()));
  completer.Reply(result);
  if (result.is_ok()) {
    parent_->ResizeVmoPool(vmo_count_ += request.vmo_ids().size());
  }
}

void ReaderInstance::UnregisterVmos(UnregisterVmosRequest& request,
                                    UnregisterVmosCompleter::Sync& completer) {
  completer.Reply(manager_.UnregisterVmos(request.vmo_ids()));
}

void ReaderInstance::StartBursts(StartBurstsCompleter::Sync& completer) {
  bursts_started_ = true;
  sent_error_ = false;
  parent_->StartBursts();
}

void ReaderInstance::StopBursts(StopBurstsCompleter::Sync& completer) {
  bursts_started_ = false;
  // We know that no more bursts will be sent on the channel after setting this flag, so it is safe
  // to immediately reply.
  completer.Reply();
  parent_->RequestStopBursts();
}

void ReaderInstance::UnlockVmo(UnlockVmoRequest& request, UnlockVmoCompleter::Sync& completer) {
  manager_.UnlockVmo(request.vmo_id());
}

void ReaderInstance::SendBurst(const cpp20::span<const uint8_t> burst, const zx::time timestamp) {
  if (!bursts_started_) {
    return;
  }

  fit::result vmo_id = manager_.WriteUnlockedVmoAndGetId(burst);
  if (vmo_id.is_error()) {
    SendError(vmo_id.error_value());
    return;
  }

  sent_error_ = false;

  const auto request = fuchsia_hardware_radar::RadarBurstReaderOnBurstRequest::WithBurst(
      {{*vmo_id, timestamp.get()}});
  if (auto event_result = fidl::SendEvent(server_)->OnBurst(request); event_result.is_error()) {
    FX_PLOGS(ERROR, event_result.error_value().status()) << "Failed to send burst";
  }
}

void ReaderInstance::SendError(const fuchsia_hardware_radar::StatusCode error) {
  if (!bursts_started_ || sent_error_) {
    return;
  }

  sent_error_ = true;

  const auto request = fuchsia_hardware_radar::RadarBurstReaderOnBurstRequest::WithError(error);
  if (auto event_result = fidl::SendEvent(server_)->OnBurst(request); event_result.is_error()) {
    FX_PLOGS(ERROR, event_result.error_value().status()) << "Failed to send burst error";
  }
}

}  // namespace radar
