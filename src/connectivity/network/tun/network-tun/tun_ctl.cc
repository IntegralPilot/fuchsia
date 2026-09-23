// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tun_ctl.h"

#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "tun_device.h"

namespace network {
namespace tun {

zx::result<std::unique_ptr<TunCtl>> TunCtl::Create(async_dispatcher_t* fidl_dispatcher) {
  std::unique_ptr<TunCtl> tun_ctl(new TunCtl(fidl_dispatcher));

  zx::result dispatchers = network::OwnedDeviceInterfaceDispatchers::Create();
  if (dispatchers.is_error()) {
    FX_PLOGST(ERROR, "tun", dispatchers.status_value()) << "failed to create owned dispatchers";
    return dispatchers.take_error();
  }
  tun_ctl->dispatchers_ = std::move(dispatchers.value());

  // Create the netdev dispatcher with a different owner, as if it was a separate driver from the
  // network device driver. This is required to allow inlining calls between dispatchers within the
  // same driver. It doesn't matter which pointer we use as the owner, as long as it doesn't clash
  // with the network device driver owner.
  zx::result netdev_dispatcher = fdf_env::DispatcherBuilder::CreateUnsynchronizedWithOwner(
      &tun_ctl->netdev_dispatcher_, {}, "netdev-dispatcher",
      [tun_ctl = tun_ctl.get()](fdf_dispatcher_t*) {
        tun_ctl->netdev_dispatcher_shutdown_.Signal();
      });
  if (netdev_dispatcher.is_error()) {
    FX_PLOGST(ERROR, "tun", netdev_dispatcher.status_value())
        << "failed to create netdev dispatcher";
    return netdev_dispatcher.take_error();
  }
  tun_ctl->netdev_dispatcher_ = std::move(netdev_dispatcher.value());

  return zx::ok(std::move(tun_ctl));
}

TunCtl::~TunCtl() {
  if (dispatchers_) {
    dispatchers_->ShutdownSync();
  }
  if (netdev_dispatcher_.get()) {
    netdev_dispatcher_.ShutdownAsync();
    netdev_dispatcher_shutdown_.Wait();
    netdev_dispatcher_.reset();
  }
}

void TunCtl::CreateDevice(CreateDeviceRequestView request, CreateDeviceCompleter::Sync& completer) {
  std::optional config = DeviceConfig::Create(request->config);
  if (!config.has_value()) {
    FX_LOGST(ERROR, "tun") << "TunCtl: Invalid DeviceConfig";
    request->device.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  zx::result tun_device = TunDevice::Create(
      dispatchers_->Unowned(), netdev_dispatcher_.borrow(),
      [this](TunDevice* dev) {
        // If this is posted on fdf_dispatcher then there's a lockup because we're
        // then creating a double lock in DevicePort.
        async::PostTask(fidl_dispatcher_, [this, dev]() {
          devices_.erase(*dev);
          TryFireShutdownCallback();
        });
      },
      std::move(config.value()));

  if (tun_device.is_error()) {
    FX_PLOGST(ERROR, "tun", tun_device.status_value()) << "TunCtl: TunDevice creation failed";
    request->device.Close(tun_device.error_value());
    return;
  }
  auto& value = tun_device.value();
  value->Bind(std::move(request->device));
  devices_.push_back(std::move(value));
  FX_LOGST(INFO, "tun") << "TunCtl: Created TunDevice";
}

void TunCtl::CreatePair(CreatePairRequestView request, CreatePairCompleter::Sync& completer) {
  std::optional config = DevicePairConfig::Create(request->config);
  if (!config.has_value()) {
    FX_LOGST(ERROR, "tun") << "TunCtl: Invalid DevicePairConfig";
    request->device_pair.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  zx::result tun_pair = TunPair::Create(
      dispatchers_->Unowned(), netdev_dispatcher_.borrow(), fidl_dispatcher_,
      [this](TunPair* pair) {
        async::PostTask(fidl_dispatcher_, [this, pair]() {
          device_pairs_.erase(*pair);
          TryFireShutdownCallback();
        });
      },
      std::move(config.value()));

  if (tun_pair.is_error()) {
    FX_PLOGST(ERROR, "tun", tun_pair.status_value()) << "TunCtl: TunPair creation failed";
    request->device_pair.Close(tun_pair.status_value());
    return;
  }
  auto& value = tun_pair.value();
  value->Bind(std::move(request->device_pair));
  device_pairs_.push_back(std::move(value));
  FX_LOGST(INFO, "tun") << "TunCtl: Created TunPair";
}

void TunCtl::SetSafeShutdownCallback(fit::callback<void()> shutdown_callback) {
  async::PostTask(fidl_dispatcher_, [this, callback = std::move(shutdown_callback)]() mutable {
    ZX_ASSERT_MSG(!shutdown_callback_, "Shutdown callback already installed");
    shutdown_callback_ = std::move(callback);
    TryFireShutdownCallback();
  });
}

void TunCtl::TryFireShutdownCallback() {
  if (shutdown_callback_ && device_pairs_.is_empty() && devices_.is_empty()) {
    shutdown_callback_();
  }
}

}  // namespace tun
}  // namespace network
