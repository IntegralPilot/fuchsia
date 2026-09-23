// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_INTERRUPT_PLIC_INCLUDE_DEV_INTERRUPT_PLIC_H_
#define ZIRCON_KERNEL_DEV_INTERRUPT_PLIC_INCLUDE_DEV_INTERRUPT_PLIC_H_

#include <zircon/compiler.h>

#include <phys/handoff.h>

__BEGIN_CDECLS

// Early and late initialization routines for the driver.
void plic_init_early(const zbi_dcfg_riscv_plic_driver_t& config);
void plic_init_post_vm(const zbi_dcfg_riscv_plic_driver_t& config);
void plic_init_late(const zbi_dcfg_riscv_plic_driver_t& config);

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_INTERRUPT_PLIC_INCLUDE_DEV_INTERRUPT_PLIC_H_
