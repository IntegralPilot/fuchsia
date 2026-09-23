// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "qemu-riscv64.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/zx/result.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/threads.h>

#include <fbl/alloc_checker.h>

namespace board_qemu_riscv64 {
namespace fpbus = fuchsia_hardware_platform_bus;

void QemuRiscv64::SysinfoInit() {
  fdf::Arena arena('RISC');
  fpbus::BootloaderInfo bootloader_info;
  bootloader_info.vendor() = "<unknown>";
  {
    fidl::Arena fidl_arena;
    auto result = pbus_.buffer(arena)->SetBootloaderInfo(fidl::ToWire(fidl_arena, bootloader_info));
    if (!result.ok()) {
      zxlogf(ERROR, "SetBoardInfo request failed: %s", result.FormatDescription().data());
    } else if (result->is_error()) {
      zxlogf(ERROR, "SetBoardInfo failed: %s", zx_status_get_string(result->error_value()));
    }
  }
}

void QemuRiscv64::DdkInit(ddk::InitTxn txn) {
  SysinfoInit();

  zx::result<> result = RtcInit();
  if (result.is_error()) {
    zxlogf(ERROR, "Couldn't initialize rtc: %s", result.status_string());
    return txn.Reply(result.error_value());
  }

  result = PcirootInit();
  if (result.is_error()) {
    zxlogf(ERROR, "Couldn't initialize pciroot: %s", result.status_string());
    return txn.Reply(result.error_value());
  }

  zxlogf(DEBUG, "Init successful");
  txn.Reply(ZX_OK);
}

zx_status_t QemuRiscv64::Create(void* ctx, zx_device_t* parent) {
  zx::result client = DdkConnectRuntimeProtocol<fpbus::Service::PlatformBus>(parent);
  if (client.is_error()) {
    return client.status_value();
  }

  fbl::AllocChecker ac;
  auto board = fbl::make_unique_checked<QemuRiscv64>(&ac, parent, std::move(client.value()));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  zx_status_t status = board->DdkAdd("qemu-riscv64", DEVICE_ADD_NON_BINDABLE);
  if (status != ZX_OK) {
    zxlogf(ERROR, "DdkAdd failed: %s", zx_status_get_string(status));
    return status;
  }

  [[maybe_unused]] auto ptr = board.release();
  return ZX_OK;
}

}  // namespace board_qemu_riscv64

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = board_qemu_riscv64::QemuRiscv64::Create;
  return ops;
}();

// clang-format off
ZIRCON_DRIVER(qemu-riscv64, driver_ops, "zircon", "0.1");
// clang-format on
