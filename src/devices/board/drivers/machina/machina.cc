// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "machina.h"

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
#include <threads.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>

#include "lib/fit/defer.h"

namespace machina {
namespace fpbus = fuchsia_hardware_platform_bus;

static void machina_board_release(void* ctx) { delete static_cast<machina_board_t*>(ctx); }

static zx_protocol_device_t machina_board_device_protocol = {
    .version = DEVICE_OPS_VERSION,
    .release = machina_board_release,
};

static const std::vector<fpbus::Mmio> pl031_mmios = {
    {{
        .base = RTC_BASE_PHYS,
        .length = RTC_SIZE,
    }},
};

static const fpbus::Node pl031_dev = []() {
  fpbus::Node dev;
  dev.name() = "pl031";
  dev.vid() = PDEV_VID_GENERIC;
  dev.pid() = PDEV_PID_GENERIC;
  dev.did() = PDEV_DID_RTC_PL031;
  dev.mmio() = pl031_mmios;
  return dev;
}();

static int machina_start_thread(void* arg) {
  machina_board_t* bus = static_cast<machina_board_t*>(arg);

  fdf::Arena arena('MACH');
  fidl::Arena<> fidl_arena;

  auto result = bus->client.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, pl031_dev));

  if (!result.ok()) {
    zxlogf(ERROR, "%s: NodeAdd request failed: %s", __func__, result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: NodeAdd failed: %s", __func__, zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

static zx_status_t machina_board_bind(void* ctx, zx_device_t* parent) {
  std::unique_ptr<machina_board_t> bus = std::make_unique<machina_board_t>();
  if (!bus) {
    return ZX_ERR_NO_MEMORY;
  }

  auto endpoints = fdf::CreateEndpoints<fpbus::PlatformBus>();
  if (endpoints.is_error()) {
    return endpoints.error_value();
  }

  zx_status_t status = device_connect_fragment_runtime_protocol(
      parent, "default", fpbus::Service::PlatformBus::ServiceName,
      fpbus::Service::PlatformBus::Name, endpoints->server.TakeHandle().release());
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to connect to platform bus: %s", zx_status_get_string(status));
    return status;
  }

  bus->client.Bind(std::move(endpoints->client));

  status = machina_pci_init(parent, bus.get());
  if (status != ZX_OK) {
    zxlogf(ERROR, "machina_pci_init failed: %d", status);
  }

  device_add_args_t args = {
      .version = DEVICE_ADD_ARGS_VERSION,
      .name = "machina",
      .ctx = bus.get(),
      .ops = &machina_board_device_protocol,
      .flags = DEVICE_ADD_NON_BINDABLE,
  };

  status = device_add(parent, &args, NULL);
  if (status != ZX_OK) {
    return status;
  }

  // The DDK takes ownership of the pointer.
  auto raw_bus = bus.release();

  thrd_t t;
  int thrd_rc = thrd_create_with_name(&t, machina_start_thread, raw_bus, "machina_start_thread");
  if (thrd_rc != thrd_success) {
    status = thrd_status_to_zx_status(thrd_rc);
    printf("machina_board_bind failed %d\n", status);
    // Since device_add succeeded, the DDK release callback will free raw_bus when the device is
    // removed.
    return status;
  }

  return status;
}

}  // namespace machina

static zx_driver_ops_t machina_board_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .bind = machina::machina_board_bind,
};

ZIRCON_DRIVER(machina_board, machina_board_driver_ops, "zircon", "0.1");
