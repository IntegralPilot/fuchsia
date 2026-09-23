// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/stdio.h>
#include <phys/uart-console.h>

#include <ktl/enforce.h>

UartDriver& GetUartDriver() {
  // This is function-local so it can be non-constinit.
  static UartDriver uart;
  return uart;
}

void SetUartConsole(const uart::all::Config<>& uart_config) {
  GetUartDriver() = uart::all::MakeDriver(uart_config);
  GetUartDriver().Visit([](auto& driver) {
    driver.Init();

    // Update stdout global to write to the configured driver.
    PhysConsole::Get().set_serial(FILE{&driver});
  });
}
