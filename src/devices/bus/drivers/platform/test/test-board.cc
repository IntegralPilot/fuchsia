// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>

#include <iterator>
#include <memory>

#include <fbl/algorithm.h>

#include "src/devices/bus/drivers/platform/test/test-metadata.h"
#include "src/devices/bus/drivers/platform/test/test-resources.h"
#include "src/devices/bus/drivers/platform/test/test.h"

namespace board_test {

void TestBoard::DdkRelease() { delete this; }

int TestBoard::Thread() {
  zx_status_t status;
  status = GpioInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: GpioInit failed: %d", __func__, status);
  }

  status = SpiInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: SpiInit failed: %d", __func__, status);
  }

  status = TestInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: TestInit failed: %d", __func__, status);
  }

  status = InterruptControllerInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: InterruptControllerInit failed: %d", __func__, status);
  }

  status = CompositeNodeSpecInit();
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: CompositeNodeSpecInit failed: %d", __func__, status);
  }

  return 0;
}

zx_status_t TestBoard::Start() {
  int rc = thrd_create_with_name(
      &thread_, [](void* arg) -> int { return reinterpret_cast<TestBoard*>(arg)->Thread(); }, this,
      "test-board-start-thread");
  if (rc != thrd_success) {
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

zx_status_t TestBoard::Create(zx_device_t* parent) {
  zx::result client =
      DdkConnectRuntimeProtocol<fuchsia_hardware_platform_bus::Service::PlatformBus>(parent);
  if (client.is_error()) {
    return client.status_value();
  }

  auto board = std::make_unique<TestBoard>(parent, std::move(client.value()));
  zx_status_t status = board->DdkAdd("test-board", DEVICE_ADD_NON_BINDABLE);
  if (status != ZX_OK) {
    zxlogf(ERROR, "TestBoard::Create: DdkAdd failed: %d", status);
    return status;
  }

  status = board->Start();
  if (status == ZX_OK) {
    // devmgr is now in charge of the device.
    [[maybe_unused]] auto* dummy = board.release();
  }
  return status;
}

zx_status_t test_bind(void* ctx, zx_device_t* parent) { return TestBoard::Create(parent); }

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = test_bind;
  return ops;
}();

}  // namespace board_test

ZIRCON_DRIVER(test_board, board_test::driver_ops, "zircon", "0.1");
