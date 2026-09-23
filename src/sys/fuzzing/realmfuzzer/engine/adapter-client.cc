// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/realmfuzzer/engine/adapter-client.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

namespace fuzzing {

TargetAdapterClient::TargetAdapterClient(ExecutorPtr executor)
    : executor_(executor), eventpair_(executor) {}

void TargetAdapterClient::Configure(const OptionsPtr& options) {
  FX_CHECK(options);
  auto max_input_size = options->max_input_size();
  if (max_input_size <= test_input_.capacity()) {
    return;
  }
  if (auto status = test_input_.Reserve(max_input_size); status != ZX_OK) {
    FX_LOGS(WARNING) << "Failed to reserve test input: " << zx_status_get_string(status);
  }
  // Since `test_input_` has been invalidated, the client will need to reconnect.
  eventpair_.Reset();
}

Promise<std::vector<std::string>> TargetAdapterClient::GetParameters() {
  Bridge<std::vector<std::string>> bridge;
  return fpromise::make_promise(
             [this, completer = std::move(bridge.completer)]() mutable -> ZxResult<> {
               if (!ptr_.is_bound()) {
                 handler_(ptr_.NewRequest(executor_->dispatcher()));
               }
               ptr_->GetParameters(completer.bind());
               return fpromise::ok();
             })
      .and_then(ConsumeBridge(bridge))
      .or_else([](const zx_status_t& status) -> Result<std::vector<std::string>> {
        FX_LOGS(ERROR) << "Failed to get parameters: " << zx_status_get_string(status);
        return fpromise::error();
      })
      .wrap_with(scope_);
}

Promise<> TargetAdapterClient::TestOneInput(const Input& test_input) {
  if (auto status = test_input_.Write(test_input.data(), test_input.size()); status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to write test input: " << zx_status_get_string(status);
    return fpromise::make_error_promise();
  }
  Bridge<> bridge;
  return fpromise::make_promise(
             [this, completer = std::move(bridge.completer)]() mutable -> ZxResult<> {
               if (eventpair_.IsConnected()) {
                 completer.complete_ok();
                 return fpromise::ok();
               }
               handler_(ptr_.NewRequest(executor_->dispatcher()));
               zx::vmo test_input;
               if (auto status = test_input_.Share(&test_input); status != ZX_OK) {
                 FX_LOGS(WARNING) << "Failed to share test input: " << zx_status_get_string(status);
                 return fpromise::error(status);
               }
               ptr_->Connect(eventpair_.Create(), std::move(test_input), completer.bind());
               return fpromise::ok();
             })
      .and_then(ConsumeBridge(bridge))
      .and_then([this]() -> ZxResult<> { return AsZxResult(eventpair_.SignalSelf(kFinish, 0)); })
      .and_then([this]() -> ZxResult<> { return AsZxResult(eventpair_.SignalPeer(0, kStart)); })
      .and_then(eventpair_.WaitFor(kFinish))
      .and_then([](const zx_signals_t& observed) -> ZxResult<> { return fpromise::ok(); })
      .or_else([](const zx_status_t& status) -> Result<> {
        if (status != ZX_ERR_PEER_CLOSED) {
          FX_LOGS(ERROR) << "Failed to test one input: " << zx_status_get_string(status);
          return fpromise::error();
        }
        return fpromise::ok();
      })
      .wrap_with(scope_);
}

void TargetAdapterClient::Disconnect() {
  eventpair_.Reset();
  ptr_.Unbind();
}

}  // namespace fuzzing
