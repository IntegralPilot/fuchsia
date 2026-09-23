// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/instrumentation_data_processor/instrumentation_data_publisher.h"

namespace instrumentation_data {

InstrumentationDataPublisher::InstrumentationDataPublisher(async_dispatcher_t* dispatcher,
                                                           VmoHandler vmo_callback)
    : dispatcher_(dispatcher), vmo_callback_(std::move(vmo_callback)) {}

void InstrumentationDataPublisher::Publish(PublishRequestView request,
                                           PublishCompleter::Sync& completer) {
  std::string data_sink(request->data_sink.data(), request->data_sink.size());
  zx::eventpair vmo_token = std::move(request->vmo_token);
  auto wait = std::make_shared<async::WaitOnce>(vmo_token.get(), ZX_EVENTPAIR_PEER_CLOSED);
  auto iterator = pending_handlers_.emplace(pending_handlers_.begin(), wait, std::move(data_sink),
                                            std::move(request->data));
  wait->Begin(dispatcher_, [this, vmo_token = std::move(vmo_token), iterator = std::move(iterator)](
                               async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                               const zx_packet_signal_t*) mutable {
    vmo_token.reset();
    auto handler = std::move(*iterator);
    pending_handlers_.erase(iterator);

    vmo_callback_(std::move(std::get<1>(handler)), std::move(std::get<2>(handler)));
  });
}

void InstrumentationDataPublisher::DrainData() {
  for (auto& handler : pending_handlers_) {
    std::get<0>(handler)->Cancel();
    vmo_callback_(std::move(std::get<1>(handler)), std::move(std::get<2>(handler)));
  }
  pending_handlers_.clear();
}

}  // namespace instrumentation_data
