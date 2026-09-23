// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_util.h"

#include <lib/sync/cpp/completion.h>

#include "src/lib/testing/predicates/status.h"

namespace network {
namespace testing {

FakeMacDeviceImpl::FakeMacDeviceImpl()
    : features_(fuchsia_hardware_network_driver::wire::Features::Builder(arena_)) {
  // setup default info
  features_.multicast_filter_count(fuchsia_hardware_network_driver::wire::kMaxMacFilter / 2);
  features_.supported_modes(netdriver::SupportedMacFilterMode::kMask);
  EXPECT_OK(zx::event::create(0, &event_));
}

fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> FakeMacDeviceImpl::Bind(
    fdf_dispatcher_t* dispatcher) {
  auto endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::MacAddr>();
  EXPECT_TRUE(!endpoints.is_error());

  binding_ = fdf::BindServer(dispatcher, std::move(endpoints->server), this);

  return std::move(endpoints->client);
}

zx::result<std::shared_ptr<MacAddrDeviceInterface>> FakeMacDeviceImpl::CreateChild(
    fdf_dispatcher_t* dispatcher) {
  auto endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::MacAddr>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  binding_ = fdf::BindServer(dispatcher, std::move(endpoints->server), this);
  libsync::Completion completion;
  std::shared_ptr<MacAddrDeviceInterface> mac;
  zx_status_t status = ZX_ERR_BAD_STATE;
  MacAddrDeviceInterface::Create(std::move(endpoints->client), dispatcher,
                                 [&](zx::result<std::shared_ptr<MacAddrDeviceInterface>> result) {
                                   status = result.status_value();
                                   if (result.is_ok()) {
                                     mac = std::move(result.value());
                                   }
                                   completion.Signal();
                                 });
  completion.Wait();

  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(mac));
}

void FakeMacDeviceImpl::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  completer.buffer(arena).Reply(mac_);
}

void FakeMacDeviceImpl::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  // Calling `Build` effectively destroys the builder. As a result, copy the
  // builder before calling `Build`.
  auto builder = features_;
  completer.buffer(arena).Reply(builder.Build());
}

void FakeMacDeviceImpl::SetMode(
    fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request, fdf::Arena& arena,
    SetModeCompleter::Sync& completer) {
  EXPECT_TRUE(IsValidMacFilterMode(request->mode));
  std::optional<netdev::wire::MacFilterMode> old_mode = mode_;
  mode_ = request->mode;
  addresses_.assign(request->multicast_macs.begin(), request->multicast_macs.end());
  if (old_mode.has_value()) {
    event_.signal(0, kConfigurationChangedEvent);
  }

  completer.buffer(arena).Reply();
}

void FakeMacDeviceImpl::Unbind() {
  if (binding_.has_value()) {
    binding_->Unbind();
    binding_.reset();
  }
}

zx_status_t FakeMacDeviceImpl::WaitConfigurationChanged() {
  zx_status_t status = event_.wait_one(kConfigurationChangedEvent, zx::time::infinite(), nullptr);
  event_.signal(kConfigurationChangedEvent, 0);
  return status;
}

}  // namespace testing
}  // namespace network
