// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>

#include <arch/riscv64/timer.h>
#include <dev/hw_watchdog/generic32/init.h>
#include <dev/init.h>
#include <dev/interrupt/plic.h>
#include <ktl/type_traits.h>
#include <ktl/variant.h>
#include <phys/arch/arch-handoff.h>

#include <ktl/enforce.h>

void PlatformDriverHandoffEarly(const ArchPhysHandoff& arch_handoff) {
  if (arch_handoff.plic_driver) {
    plic_init_early(*arch_handoff.plic_driver.to_std());
  }

  if (arch_handoff.generic_timer_driver) {
    riscv_generic_timer_init_early(*arch_handoff.generic_timer_driver.to_std());
  }
}

void PlatformDriverHandoffPostVm(const ArchPhysHandoff& arch_handoff) {
  if (arch_handoff.plic_driver) {
    plic_init_post_vm(*arch_handoff.plic_driver.to_std());
  }
}

void PlatformDriverHandoffLate(const ArchPhysHandoff& arch_handoff) {
  if (arch_handoff.plic_driver) {
    plic_init_late(*arch_handoff.plic_driver.to_std());
  }
}
