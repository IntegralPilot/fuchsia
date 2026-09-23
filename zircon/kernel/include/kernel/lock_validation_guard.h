// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_VALIDATION_GUARD_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_VALIDATION_GUARD_H_

#include <arch/interrupt.h>

// Injects interrupt save/restore into lockdep validation to make thread-local lock list mutations
// atomic in locks that do not already impose interrupts off/save/restore.
class LockValidationGuard {
 public:
  LockValidationGuard() : interrupt_state_{arch_interrupt_save()} {}
  ~LockValidationGuard() { arch_interrupt_restore(interrupt_state_); }

  LockValidationGuard(const LockValidationGuard&) = delete;
  LockValidationGuard& operator=(const LockValidationGuard&) = delete;

 private:
  const interrupt_saved_state_t interrupt_state_;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_VALIDATION_GUARD_H_
