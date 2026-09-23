// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "qemu-bus.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/threads.h>

#include <fbl/alloc_checker.h>

namespace board_qemu_arm64 {
namespace fpbus = fuchsia_hardware_platform_bus;

int QemuArm64::Thread() {
  zx_status_t status;
  zxlogf(INFO, "qemu-bus thread running ");

  status = PciInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: PciInit() failed %d", __func__, status);
    return thrd_error;
  }

  status = PciAdd();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: PciAdd() failed %d", __func__, status);
    return thrd_error;
  }

  status = RtcInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: RtcInit() failed %d", __func__, status);
    return thrd_error;
  }

  return 0;
}

zx_status_t QemuArm64::Start() {
  auto cb = [](void* arg) -> int { return reinterpret_cast<QemuArm64*>(arg)->Thread(); };
  int rc = thrd_create_with_name(&thread_, cb, this, "qemu-arm64");
  return thrd_status_to_zx_status(rc);
}

zx_status_t QemuArm64::Create(void* ctx, zx_device_t* parent) {
  zx::result client = DdkConnectRuntimeProtocol<fpbus::Service::PlatformBus>(parent);
  if (client.is_error()) {
    return client.status_value();
  }

  fbl::AllocChecker ac;
  auto board = fbl::make_unique_checked<QemuArm64>(&ac, parent, std::move(client.value()));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  if (zx_status_t status = board->DdkAdd("qemu-bus", DEVICE_ADD_NON_BINDABLE); status != ZX_OK) {
    zxlogf(ERROR, "%s: DdkAdd failed %d", __func__, status);
    return status;
  }

  if (zx_status_t status = board->Start(); status != ZX_OK) {
    return status;
  }

  [[maybe_unused]] auto* dummy = board.release();
  return ZX_OK;
}

}  // namespace board_qemu_arm64

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = board_qemu_arm64::QemuArm64::Create;
  return ops;
}();

// clang-format off
ZIRCON_DRIVER(qemu-arm64, driver_ops, "zircon", "0.1");
//clang-format on
