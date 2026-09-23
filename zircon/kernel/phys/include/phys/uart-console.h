// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_UART_CONSOLE_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_UART_CONSOLE_H_

#include <lib/uart/all.h>

#include "uart.h"

// Wires up the associated UART to stdout via PhysConsole::set_serial.
void SetUartConsole(const uart::all::Config<>& uart_config);

// Gets the underlying driver set up by SetUartConsole.  The reference
// returned is always the same and can be kept across SetUartConsole calls.
UartDriver& GetUartDriver();

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_UART_CONSOLE_H_
