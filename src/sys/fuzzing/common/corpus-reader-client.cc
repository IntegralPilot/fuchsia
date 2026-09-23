// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/corpus-reader-client.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "src/sys/fuzzing/common/async-socket.h"

namespace fuzzing {

CorpusReaderClient::CorpusReaderClient(ExecutorPtr executor) : executor_(std::move(executor)) {}

fidl::InterfaceRequest<CorpusReader> CorpusReaderClient::NewRequest() {
  return ptr_.NewRequest(executor_->dispatcher());
}

void CorpusReaderClient::Bind(fidl::InterfaceHandle<CorpusReader> handle) {
  auto status = ptr_.Bind(std::move(handle), executor_->dispatcher());
  FX_CHECK(status == ZX_OK) << zx_status_get_string(status);
}

ZxPromise<> CorpusReaderClient::Send(std::vector<Input> inputs) {
  // Create sockets for all non-empty inputs, plus one final empty sentinel input.
  std::deque<FidlInput> fidl_inputs;
  for (const auto& input : inputs) {
    if (input.size()) {
      fidl_inputs.emplace_back(AsyncSocketWrite(executor_, input));
    }
  }
  fidl_inputs.emplace_back(AsyncSocketWrite(executor_, Input()));
  // Create a future which sends data to each socket in turn.
  ZxBridge<> outer;
  auto task =
      fpromise::make_promise([this, fidl_inputs = std::move(fidl_inputs),
                              sending = ZxFuture<>()](Context& context) mutable -> ZxResult<> {
        while (true) {
          if (fidl_inputs.empty()) {
            return fpromise::ok();
          }
          if (!sending) {
            Bridge<zx_status_t> inner;
            ptr_->Next(std::move(fidl_inputs.front()), inner.completer.bind());
            sending = ConsumeBridge(inner);
          }
          if (!sending(context)) {
            return fpromise::pending();
          }
          if (sending.is_error()) {
            return fpromise::error(sending.error());
          }
          sending = nullptr;
          fidl_inputs.pop_front();
        }
      })
          .then([callback = ZxBind<>(std::move(outer.completer))](ZxResult<>& result) {
            if (result.is_error()) {
              FX_LOGS(WARNING) << "Failed to send corpus: " << zx_status_get_string(result.error());
            }
            callback(result);
          })
          .wrap_with(scope_);
  executor_->schedule_task(std::move(task));
  return ConsumeBridge(outer);
}

}  // namespace fuzzing
