// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_POWER_MOTMOT_INCLUDE_DEV_POWER_MOTMOT_MOTMOT_FFI_H_
#define ZIRCON_KERNEL_DEV_POWER_MOTMOT_INCLUDE_DEV_POWER_MOTMOT_MOTMOT_FFI_H_

#include <stdint.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

uint64_t cpp_motmot_modify_register_via_smc(uintptr_t phys_addr, uint32_t mask, uint32_t val);
__NO_RETURN void cpp_motmot_cpu_off_wfi_loop();

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_POWER_MOTMOT_INCLUDE_DEV_POWER_MOTMOT_MOTMOT_FFI_H_
