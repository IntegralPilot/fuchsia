// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_PHYS_LEGACY_BOOT_H_
#define ZIRCON_KERNEL_ARCH_X86_PHYS_LEGACY_BOOT_H_

#include <lib/boot-shim/tty.h>
#include <lib/uart/all.h>
#include <lib/zbi-format/memory.h>

#include <cstdint>
#include <optional>
#include <span>
#include <string_view>

class AddressSpace;

// This holds information collected from a legacy boot loader protocol.
struct LegacyBoot {
  std::string_view bootloader;
  std::string_view cmdline;
  std::span<std::byte> ramdisk;
  std::span<zbi_mem_range_t> mem_config;
  uint64_t acpi_rsdp = 0;  // Physical address of the ACPI RSDP.
  uint64_t smbios = 0;     // Physical address of the SMBIOS table.
  uart::all::Config<> uart_config;
};

// InitMemory() initializes this.
//
// The space pointed to by the members is safe from reclamation by the memory
// allocator after InitMemory().
extern LegacyBoot gLegacyBoot;

// InitMemory() calls this to adjust gLegacyBoot before using its data.
// It need not be defined.
[[gnu::weak]] void LegacyBootQuirks();

// Wires up the associated UART to stdout, and possibly finishes initializing
// it (which in the non-legacy case is assumed to be properly done by the
// bootloader).
void LegacyBootSetUartConsole(const uart::all::Config<>& uart);

// This is a subroutine of InitMemory().  It primes the allocator and reserves
// ranges based on the data in gLegacyBoot, then sets up paging.
void LegacyBootInitMemory(AddressSpace* aspace);

// Returns a legacy uart Pio driver from `tty` or `std::nullopt` if `tty` does not match a valid COM
// port.
//
// This is meant to be used in legacy system relying on Port I/O instructions.`tty.index` is matched
// against known COM ports.
//
// `tty.type` must be either `kSerial` or `kAny`.
//
std::optional<uart::ns8250::PioDriver> LegacyUartFromTty(const boot_shim::Tty& tty);

#endif  // ZIRCON_KERNEL_ARCH_X86_PHYS_LEGACY_BOOT_H_
