// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_PDEV_POWER_INCLUDE_PDEV_POWER_H_
#define ZIRCON_KERNEL_DEV_PDEV_POWER_INCLUDE_PDEV_POWER_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <dev/power.h>
#include <kernel/cpu.h>

// power interface
struct pdev_power_ops {
  zx_status_t (*reboot)(power_reboot_flags flags);
  zx_status_t (*shutdown)();
  zx_status_t (*cpu_off)();
  zx_status_t (*cpu_on)(uint64_t hw_cpu_id, paddr_t entry, uint64_t context);
  zx_status_t (*get_cpu_state)(uint64_t hw_cpu_id, power_cpu_state* out_state);
  zx_status_t (*opp_set)(uint32_t domain_id, uint64_t opp);
  zx_status_t (*opp_get)(uint32_t domain_id, uint64_t* out_opp);
  zx_status_t (*opp_get_domain_count)(size_t* out_count);
};

static_assert(sizeof(pdev_power_ops) == 64, "pdev_power_ops size mismatch");
static_assert(alignof(pdev_power_ops) == 8, "pdev_power_ops align mismatch");

void pdev_register_power(const pdev_power_ops* ops);

const pdev_power_ops* pdev_swap_power_for_test(const pdev_power_ops* ops);

__BEGIN_CDECLS

void cpp_rppm_dump();
zx_status_t cpp_rppm_update_active_power_level(cpu_num_t cpu, uint8_t power_level);
bool cpp_rppm_request_power_level_for_testing(cpu_num_t cpu, uint8_t power_level);
zx_status_t cpp_rppm_get_active_power_level(cpu_num_t cpu, uint8_t* out_power_level);
zx_status_t cpp_rppm_update_processing_limits(uint64_t cpu_mask, uint64_t min_rate,
                                              uint64_t max_rate);
size_t cpp_rppm_get_processor_count();

__END_CDECLS

#endif  // ZIRCON_KERNEL_DEV_PDEV_POWER_INCLUDE_PDEV_POWER_H_
