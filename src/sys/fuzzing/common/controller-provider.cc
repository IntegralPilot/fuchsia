// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/controller-provider.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

namespace fuzzing {

using ::fuchsia::fuzzer::RegistrySyncPtr;

ControllerProviderImpl::ControllerProviderImpl(RunnerPtr runner)
    : binding_(this), controller_(std::move(runner)) {
  binding_.set_error_handler([](zx_status_t status) {
    // The registry signals the provider should exit by closing its channel.
    if (status != ZX_ERR_PEER_CLOSED) {
      FX_LOGS(WARNING) << "Registry connection closed: " << zx_status_get_string(status);
      exit(status);
    }
    exit(0);
  });
}

///////////////////////////////////////////////////////////////
// FIDL methods

void ControllerProviderImpl::Connect(fidl::InterfaceRequest<Controller> request,
                                     ConnectCallback callback) {
  controller_.Bind(std::move(request));
  callback();
}

void ControllerProviderImpl::Stop() { controller_.Stop(); }

///////////////////////////////////////////////////////////////
// Run-related methods

Promise<> ControllerProviderImpl::Serve(const std::string& url, zx::channel channel) {
  FX_CHECK(channel);
  Bridge<> bridge;
  return fpromise::make_promise([this, url = std::string(url), channel = std::move(channel),
                                 completer = std::move(bridge.completer)]() mutable -> Result<> {
           auto provider = binding_.NewBinding();
           registrar_.Bind(std::move(channel));
           registrar_->Register(url, std::move(provider), completer.bind());
           return fpromise::ok();
         })
      .and_then(bridge.consumer.promise_or(fpromise::error()))
      .wrap_with(scope_);
}

}  // namespace fuzzing
