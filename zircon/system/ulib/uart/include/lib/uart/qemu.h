// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_UART_QEMU_H_
#define LIB_UART_QEMU_H_

// QEMU-only tests and boot shims hard-code a particular driver configuration.

#include "ns8250.h"
#include "null.h"
#include "pl011.h"
#include "uart.h"

namespace uart::qemu {

// uart::qemu::kConfig is configuration object appropriate for QEMU environments.

#ifdef __aarch64__

static constexpr uart::Config<pl011::Driver> kConfig{pl011::kQemuConfig};

#elif defined(__x86_64__) || defined(__i386__)

static constexpr uart::Config<ns8250::PioDriver> kConfig{ns8250::kLegacyConfig};

#elif defined(__riscv)

static constexpr uart::Config<ns8250::Mmio8Driver> kConfig{{
    .mmio_phys = 0x10000000,
    .irq = 10,
}};

#else

static constexpr uart::Config<null::Driver> kConfig{};

#endif

}  // namespace uart::qemu

#endif  // LIB_UART_QEMU_H_
