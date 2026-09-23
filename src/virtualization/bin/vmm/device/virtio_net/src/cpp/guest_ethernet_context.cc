// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/bin/vmm/device/virtio_net/src/cpp/guest_ethernet_context.h"

#include <lib/fdf/cpp/env.h>
#include <lib/syslog/cpp/macros.h>

zx::result<std::unique_ptr<GuestEthernetContext>> GuestEthernetContext::Create() {
  zx_status_t status = fdf_env_start(0);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::unique_ptr<GuestEthernetContext> context(new GuestEthernetContext);

  fdf_env_register_driver_entry(context.get());

  zx::result sync_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "guest-ethernet-sync-dispatcher",
      [context = context.get()](fdf_dispatcher_t*) {
        context->sync_dispatcher_shutdown_.Signal();
      });
  if (sync_dispatcher.is_error()) {
    FX_LOGS(ERROR) << "Failed to create impl dispatcher: " << sync_dispatcher.status_string();
    return sync_dispatcher.take_error();
  }
  context->sync_dispatcher_ = std::move(sync_dispatcher.value());

  zx::result dispatchers = network::OwnedDeviceInterfaceDispatchers::Create();
  if (dispatchers.is_error()) {
    FX_LOGS(ERROR) << "Failed to create netdev dispatchers: " << dispatchers.status_string();
    return dispatchers.take_error();
  }
  context->dispatchers_ = std::move(dispatchers.value());

  return zx::ok(std::move(context));
}

GuestEthernetContext::~GuestEthernetContext() {
  if (sync_dispatcher_.get()) {
    sync_dispatcher_.ShutdownAsync();
    sync_dispatcher_shutdown_.Wait();
  }
  if (dispatchers_) {
    dispatchers_->ShutdownSync();
  }

  fdf_env_register_driver_exit();
  fdf_env_reset();
}
