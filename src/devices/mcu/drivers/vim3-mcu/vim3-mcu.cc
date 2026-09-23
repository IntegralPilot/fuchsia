// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vim3-mcu.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <zircon/compiler.h>

#include <iterator>

#include <ddktl/fidl.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

#include "src/devices/i2c/lib/i2c-channel-legacy/i2c-channel.h"

namespace stm {
zx_status_t StmMcu::Create(void* ctx, zx_device_t* parent) {
  ddk::I2cChannel i2c(parent, "i2c");
  if (!i2c.is_valid()) {
    zxlogf(ERROR, "Failed to get ZX_PROTOCOL_I2C");
    return ZX_ERR_NO_RESOURCES;
  }

  fbl::AllocChecker ac;
  std::unique_ptr<StmMcu> device(new (&ac) StmMcu(parent, std::move(i2c)));
  if (!ac.check()) {
    zxlogf(ERROR, "StmMcu alloc failed");
    return ZX_ERR_NO_MEMORY;
  }

  device->Init();

  zx::result result = device->DdkAddService<fuchsia_hardware_fan::Service>(
      fuchsia_hardware_fan::Service::InstanceHandler({
          .device = device->bindings_.CreateHandler(
              device.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              fidl::kIgnoreBindingClosure),
      }));
  if (result.is_error()) {
    zxlogf(ERROR, "DdkAddService failed");
    return result.status_value();
  }

  if (zx_status_t status =
          device->DdkAdd(ddk::DeviceAddArgs("vim3-mcu").set_inspect_vmo(device->inspect_vmo()));
      status != ZX_OK) {
    zxlogf(ERROR, "DdkAdd failed");
    return status;
  }

  [[maybe_unused]] auto* dummy = device.release();

  return ZX_OK;
}
void StmMcu::ShutDown() {}

void StmMcu::DdkUnbind(ddk::UnbindTxn txn) {
  ShutDown();
  txn.Reply();
}
void StmMcu::DdkRelease() { delete this; }

void StmMcu::Init() {
  SetFanLevel(static_cast<FanLevel>(VIM3_MCU_FAN_DEFAULT_LEVEL));

  const uint8_t wol_reset_enable[] = {STM_MCU_REG_BOOT_EN_WOL, STM_MCU_REG_EN_WOL_RESET_ENABLE};
  zx_status_t status = i2c_.WriteSync(wol_reset_enable, std::size(wol_reset_enable));
  if (status != ZX_OK) {
    zxlogf(WARNING, "Failed to enable WOL: %d", status);
  }

  uint8_t pcie_enabled = 0;
  status = i2c_.ReadSync(STM_MCU_USB_PCIE_SWITCH_REG, &pcie_enabled, 1);
  if (status != ZX_OK) {
    zxlogf(WARNING, "Failed to read PCIe enabled: %s", zx_status_get_string(status));
    pcie_enabled = -1;
  }

  if (pcie_enabled != 0) {
    zxlogf(INFO, "PCIe is enabled. Switching it to USB3.");
    pcie_enabled = 0;
    const uint8_t write_buf[] = {STM_MCU_USB_PCIE_SWITCH_REG, pcie_enabled};
    status = i2c_.WriteSync(write_buf, std::size(write_buf));
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to disable PCIe: %s", zx_status_get_string(status));
      pcie_enabled = 1;
    }
  }

  inspect_.GetRoot().CreateUint("PCIe enabled", pcie_enabled, &inspect_);
}

zx_status_t StmMcu::SetFanLevel(FanLevel level) {
  fbl::AutoLock lock(&i2c_lock_);
  uint8_t cmd[] = {STM_MCU_REG_CMD_FAN_STATUS_CTRL_REG, static_cast<uint8_t>(level)};
  auto status = i2c_.WriteSync(cmd, sizeof(cmd));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Could not set the fan level: %d", status);
    return status;
  }
  fan_level_ = level;
  return ZX_OK;
}

static constexpr zx_driver_ops_t stm_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = StmMcu::Create;
  return ops;
}();

}  // namespace stm

ZIRCON_DRIVER(vim3_mcu, stm::stm_driver_ops, "zircon", "0.1");
