// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "stdout.h"

#include <lib/boot-options/boot-options.h>
#include <lib/boot-options/word-view.h>
#include <lib/boot-shim/tty.h>
#include <lib/uart/all.h>

#include <phys/boot-options.h>
#include <phys/uart.h>

#include "../legacy-boot.h"

// Pure Multiboot loaders like QEMU provide no means of information about the
// serial port, just the command line.  So parse it just for kernel.serial.
void UartFromCmdLine(ktl::string_view cmdline, uart::all::Config<>& uart_config) {
  BootOptions boot_opts;
  boot_opts.serial = uart_config;

  // `console=` command-line option will override settings provided by ACPI or ZBI items,
  // and this option will be overriden by `kernel.serial=` option.
  if (auto tty = boot_shim::TtyFromCmdline(cmdline)) {
    if (auto legacy_uart = LegacyUartFromTty(*tty)) {
      boot_opts.serial = *legacy_uart;
    }
  }

  SetBootOptionsWithoutEntropy(boot_opts, {}, cmdline);
  uart_config = boot_opts.serial;
}
