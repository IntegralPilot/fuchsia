// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/root_resource_filter.h>
#include <lib/uart/uart.h>
#include <lib/zbi-format/driver-config.h>
#include <zircon/syscalls/resource.h>

#include <cstdint>

#include <arch/x86/apic.h>
#include <ktl/optional.h>
#include <platform/uart.h>
#include <vm/physmap.h>

ktl::optional<uint32_t> PlatformUartGetIrqNumber(uint32_t irq_num) {
  if (irq_num == 0) {
    return ktl::nullopt;
  }
  return apic_io_isa_to_global(static_cast<uint8_t>(irq_num));
}
