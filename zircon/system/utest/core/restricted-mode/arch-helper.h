// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// This header defines what each architecture implementation
// will need to provide for testing.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_HELPER_H_
#define ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_HELPER_H_

#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/register-state.h>

// The normal-mode view of restricted-mode state will change slightly depending on
// if the exit to normal-mode was caused by a syscall or an exception.
enum class RegisterMutation {
  kFromSyscall,
  kFromException,
};

class ArchHelper {
 public:
  ArchHelper() = default;
  virtual ~ArchHelper() = default;
  virtual void SetInitialState(restricted_machine::RegisterState *state) const;
  virtual void VerifyStateMutation(restricted_machine::RegisterState *state,
                                   RegisterMutation mutation) const;
  virtual void VerifyState(restricted_machine::RegisterState *state) const;
};

// Returns the correct ArchHelper for the machine type
class ArchHelperFactory {
 public:
  static std::unique_ptr<ArchHelper> Create(restricted_machine::MachineType machine_type);
};

#endif  // ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_HELPER_H_
