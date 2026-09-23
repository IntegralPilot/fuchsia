// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_RESTRICTED_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_RESTRICTED_H_

#include <debug.h>
#include <zircon/errors.h>

struct ArchSavedNormalState {
  // Restricted Mode EL0 has its own TPIDR so we must save/restore Normal Mode's.
  uint64_t tpidr_el0 = 0;
  // Also save/restore TPIDRRO_EL0, as Restricted Mode EL0 mirrors TPIDR_EL0.
  uint64_t tpidrro_el0 = 0;
};

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_RESTRICTED_H_
