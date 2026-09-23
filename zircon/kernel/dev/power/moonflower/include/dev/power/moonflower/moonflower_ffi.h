// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_MOONFLOWER_FFI_H_
#define ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_MOONFLOWER_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS

uintptr_t cpp_moonflower_get_opp_vaddr();
int64_t cpp_moonflower_tz_config_hw_for_ram_dump(uint64_t disable_wd_dbg,
                                                 uint64_t boot_partition_sel);
int64_t cpp_moonflower_tz_io_write(paddr_t paddr, uint32_t val);

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_POWER_MOONFLOWER_INCLUDE_DEV_POWER_MOONFLOWER_MOONFLOWER_FFI_H_
