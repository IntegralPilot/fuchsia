// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "examples/drivers/simple/dfv1/simple_driver.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>

namespace simple {

SimpleDriver::SimpleDriver(zx_device_t* parent) : DeviceType(parent) {
  zxlogf(INFO,
         "SimpleDriver's constructor invoked. This constructor is only implemented to"
         "demonstrate the DFv1 driver lifecycle.");
}

SimpleDriver::~SimpleDriver() {
  zxlogf(INFO, "SimpleDriver's destructor invoked immediately after DdkRelease() or DdkSuspend().");
}

zx_status_t SimpleDriver::Bind(void* ctx, zx_device_t* dev) {
  zxlogf(INFO,
         "SimpleDriver's bind hook invoked. This signals that the driver is binding to "
         "a node. The driver is expected to add a new child in this function.");
  auto driver = std::make_unique<SimpleDriver>(dev);

  zx_status_t status = driver->DdkAdd("simple_child");
  if (status != ZX_OK) {
    return status;
  }
  // The Driver Framework now owns driver.
  [[maybe_unused]] auto ptr = driver.release();
  return ZX_OK;
}

void SimpleDriver::DdkInit(ddk::InitTxn txn) {
  zxlogf(INFO,
         "DdkInit() invoked. This signals that the driver is loaded into a Driver Host "
         "process and allows for any global initialization. Typically none is required.");
  txn.Reply(ZX_OK);
}

void SimpleDriver::DdkSuspend(ddk::SuspendTxn txn) {
  zxlogf(INFO,
         "DdkSuspend() invoked. This is only called in the Driver Framework's reboot, shutdown, "
         "or mexec codepaths. If it's invoked, then DdkUnbind() and DdkRelease() won't be called. "
         "Typically used to unmap DMA regions and unpin all DMA-related VMOs.");
  txn.Reply(ZX_OK, 0);
}

void SimpleDriver::DdkUnbind(ddk::UnbindTxn txn) {
  zxlogf(
      INFO,
      "DdkUnbind() invoked. This signals to the driver it should start shutting the device down. "
      "Only called if the driver is getting unbound.");
  txn.Reply();
}

void SimpleDriver::DdkRelease() {
  zxlogf(INFO,
         "DdkRelease() invoked. This signals that the driver has completed unbinding, "
         "all open instances of that device have been closed, and all children of"
         "that device have been unbound and released.");
  delete this;
}

static zx_driver_ops_t simple_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = SimpleDriver::Bind;
  return ops;
}();

}  // namespace simple

ZIRCON_DRIVER(SimpleDriver, simple::simple_driver_ops, "zircon", "0.1");
