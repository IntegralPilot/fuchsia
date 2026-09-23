// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_PHYS_BOOT_SHIM_LEGACY_BOOT_SHIM_H_
#define ZIRCON_KERNEL_ARCH_X86_PHYS_BOOT_SHIM_LEGACY_BOOT_SHIM_H_

#include <lib/acpi_lite.h>
#include <lib/boot-shim/acpi.h>
#include <lib/boot-shim/boot-shim.h>
#include <lib/boot-shim/pool-mem-config.h>
#include <lib/boot-shim/test-serial-number.h>
#include <lib/boot-shim/uart.h>
#include <lib/uart/all.h>
#include <stdio.h>

#include <optional>
#include <span>

#include "../legacy-boot.h"

// Must be defined by each legacy shim.
extern const char* kLegacyShimName;

class BootZbi;

using SmbiosItem = boot_shim::SingleOptionalItem<uint64_t, ZBI_TYPE_SMBIOS>;

using LegacyBootShimBase = boot_shim::BootShim<  //
    boot_shim::PoolMemConfigItem,                //
    boot_shim::UartItem<>,                       //
    boot_shim::AcpiRsdpItem,                     //
    SmbiosItem,                                  //
    boot_shim::TestSerialNumberItem>;

class LegacyBootShim : public LegacyBootShimBase {
 public:
  LegacyBootShim(const char* name, const LegacyBoot& info, FILE* log = stdout)
      : LegacyBootShimBase(name, log), input_zbi_(std::as_bytes(info.ramdisk)) {
    set_info(info.bootloader);
    set_cmdline(info.cmdline);
    Log(input_zbi_.storage());
    Check("Error scanning ZBI", Get<SerialNumber>().Init(input_zbi_));
    if (info.acpi_rsdp != 0) {
      fprintf(log, "%s:   ACPI RSDP @ %#" PRIx64 "\n", name, info.acpi_rsdp);
      Get<boot_shim::AcpiRsdpItem>().set_payload(info.acpi_rsdp);
    } else {
      fprintf(log, "%s:   ACPI RSDP not found\n", name);
    }
    if (info.smbios != 0) {
      fprintf(log, "%s:   SMBIOS @ %#" PRIx64 "\n", name, info.smbios);
      Get<SmbiosItem>().set_payload(info.smbios);
    } else {
      fprintf(log, "%s:   SMBIOS not found\n", name);
    }
    Get<boot_shim::UartItem<>>().Init(info.uart_config);
  }

  void InitMemConfig(const memalloc::Pool& pool) { Get<boot_shim::PoolMemConfigItem>().Init(pool); }

  InputZbi& input_zbi() { return input_zbi_; }

  bool Load(BootZbi& boot);

 private:
  using SerialNumber = boot_shim::TestSerialNumberItem;

  bool StandardLoad(BootZbi& boot);

  // This gets first crack before StandardLoad.
  // If it returns false then StandardLoad  is done.
  bool BootQuirksLoad(BootZbi& boot);

  // Helper for BootQuirksLoad to recognize an apparently valid bootable ZBI
  // (or a simply empty one, which can get the standard error path).
  bool IsProperZbi() const;

  InputZbi input_zbi_;
};

// If |zbi| contains a uart driver, |uart| is overwritten with such configuration.
uart::all::Config<> UartFromZbi(LegacyBootShim::InputZbi zbi,
                                const uart::all::Config<>& uart_config);

std::optional<uart::all::Config<>> GetUartFromRange(LegacyBootShim::InputZbi::iterator start,
                                                    LegacyBootShim::InputZbi::iterator end);

#endif  // ZIRCON_KERNEL_ARCH_X86_PHYS_BOOT_SHIM_LEGACY_BOOT_SHIM_H_
