// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/runtime-composite-test/drivers/composite-driver.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>

namespace frct = fuchsia_runtime_composite_test;

namespace composite_driver {

// static
zx_status_t CompositeDriver::Bind(void* ctx, zx_device_t* device) {
  auto dev = std::make_unique<CompositeDriver>(device);

  zx_status_t status = dev->DdkAdd("composite");
  if (status != ZX_OK) {
    return status;
  }

  [[maybe_unused]] auto ptr = dev.release();
  return ZX_OK;
}

void CompositeDriver::DdkInit(ddk::InitTxn init_txn) {
  // Connect to our parent driver's driver transport protocol.
  auto client_end =
      DdkConnectFragmentRuntimeProtocol<frct::Service::RuntimeCompositeProtocol>("test_primary");
  if (client_end.is_error()) {
    zxlogf(ERROR, "DdkConnectRuntimeProtocol failed");
    init_txn.Reply(client_end.status_value());
    return;
  }

  auto* dispatcher = fdf_dispatcher_get_current_dispatcher();
  client_ = fdf::Client<frct::RuntimeCompositeProtocol>(std::move(*client_end), dispatcher);

  client_->Handshake().ThenExactlyOnce(
      [init_txn = std::move(init_txn)](
          fdf::Result<frct::RuntimeCompositeProtocol::Handshake>& result) mutable {
        ZX_ASSERT(result.is_ok());
        init_txn.Reply(ZX_OK);
      });
}

static zx_driver_ops_t kDriverOps = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = CompositeDriver::Bind;
  return ops;
}();

}  // namespace composite_driver

ZIRCON_DRIVER(composite_driver, composite_driver::kDriverOps, "zircon", "0.1");
