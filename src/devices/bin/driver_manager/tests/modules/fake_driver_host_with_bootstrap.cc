// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/module.h>
#include <lib/zx/channel.h>
#include <stdint.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include "deps.h"
#include "driver_entry_point.h"
#include "entry_point.h"
#include "fake_runtime.h"
#include "v1_driver_entry_point.h"

// Returns the function address of the first instance of |symbol|.
uint64_t FindSymbol(ld::abi::Abi<>* dl_passive_abi, const char* symbol) {
  const auto& modules = ld::AbiLoadedModules<>(*dl_passive_abi);
  if (modules.empty()) {
    return 0;
  }
  auto executable_module = modules.begin();
  while (executable_module != modules.end()) {
    const elfldltl::SymbolName kDriverStart{symbol};
    auto* symbol = kDriverStart.Lookup(executable_module->symbols);
    if (symbol) {
      auto load_bias = executable_module->link_map.addr;
      return load_bias + symbol->value;
    }
    executable_module++;
  }
  return 0;
}

// This fake driver host waits for messages to be received on the bootstrap channel.
// Each message will contain the dynamic linking passive ABI for the loaded module,
// which can be queried to find the driver's start function symbol.
// If the driver host receives a null address, the driver host will exit and
// return the summation of the return values from calling each driver start function.
extern "C" int64_t Start(zx_handle_t bootstrap, void* vdso) {
  zx::channel channel{bootstrap};

  zx_signals_t signals;
  int64_t total = 0;
  while (true) {
    // Wait for the driver manager to send the address of the fake driver's DriverStart()
    // function.
    zx_status_t status = channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &signals);
    if (status != ZX_OK) {
      return status;
    }
    if (!(signals & ZX_CHANNEL_READABLE)) {
      return ZX_ERR_BAD_STATE;
    }

    uintptr_t ptr;
    uint32_t actual_bytes, actual_handles;
    status = channel.read(0, &ptr, nullptr, sizeof(ptr), 0, &actual_bytes, &actual_handles);
    if (status != ZX_OK) {
      return status;
    }
    if (actual_bytes != sizeof(ptr)) {
      return ZX_ERR_BUFFER_TOO_SMALL;
    }
    // Received a null address, signalling that the driver host should exit.
    if (ptr == 0) {
      return total;
    }

    auto dl_passive_abi = reinterpret_cast<ld::abi::Abi<>*>(ptr);
    const auto driver_start_func_addr = FindSymbol(dl_passive_abi, "DriverStart");
    if (!driver_start_func_addr) {
      return ZX_ERR_NOT_FOUND;
    }
    const auto driver_start_func = reinterpret_cast<decltype(DriverStart)*>(driver_start_func_addr);

    // This only applies for compat drivers, not an error if we can't find this.
    const auto v1_driver_start_func_addr = FindSymbol(dl_passive_abi, "V1DriverStart");

    // If present, send the address of the v1 driver start to the compat driver.
    int data_len = 0;
    uint64_t data[1];
    if (v1_driver_start_func_addr != 0) {
      data[0] = v1_driver_start_func_addr;
      data_len = 1;
    }
    total += driver_start_func(data, data_len);
  }
  return total;
}

// Implementation of fake_runtime.h.
__EXPORT int64_t runtime_get_id() { return a() + 1; }
