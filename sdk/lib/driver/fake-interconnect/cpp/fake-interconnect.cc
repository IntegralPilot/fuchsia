// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-interconnect/cpp/fake-interconnect.h>
#include <lib/fit/result.h>
#include <zircon/errors.h>

namespace fdf_fake {

FakeInterconnect::FakeInterconnect(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

fuchsia_hardware_interconnect::PathService::InstanceHandler FakeInterconnect::GetInstanceHandler() {
  return fuchsia_hardware_interconnect::PathService::InstanceHandler({
      .path = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
  });
}

fidl::ClientEnd<fuchsia_hardware_interconnect::Path> FakeInterconnect::Connect() {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_interconnect::Path>::Create();
  bindings_.AddBinding(dispatcher_, std::move(endpoints.server), this, fidl::kIgnoreBindingClosure);
  return std::move(endpoints.client);
}

void FakeInterconnect::SetBandwidth(SetBandwidthRequest& request,
                                    SetBandwidthCompleter::Sync& completer) {
  bandwidth_bps_ = {request.average_bandwidth_bps(), request.peak_bandwidth_bps()};
  completer.Reply(fit::success());
}

void FakeInterconnect::NotImplemented_(const std::string& name, fidl::CompleterBase& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeInterconnect::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_interconnect::Path> md,
    fidl::UnknownMethodCompleter::Sync& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

std::pair<std::optional<uint64_t>, std::optional<uint64_t>> FakeInterconnect::bandwidth_bps() {
  auto bandwith_bps = bandwidth_bps_;
  bandwidth_bps_ = {};
  return bandwith_bps;
}

}  // namespace fdf_fake
