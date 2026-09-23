// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/stack.h>

BootStack boot_stack, phys_exception_stack;
BootUnsafeStack boot_unsafe_stack, phys_exception_unsafe_stack;
BootShadowCallStack boot_shadow_call_stack, phys_exception_shadow_call_stack;

// This considers the limit to be "on".
bool BootStack::IsOnStack(uintptr_t sp) const {
  const uintptr_t base = reinterpret_cast<uintptr_t>(stack);
  return base <= sp && sp - base <= sizeof(stack);
}

#if __has_feature(shadow_call_stack)

arch::ShadowCallStackBacktrace BootShadowCallStack::BackTrace(uintptr_t scsp) const {
  return arch::ShadowCallStackBacktrace(shadow_call_stack, scsp);
}

#endif  // __has_feature(shadow_call_stack)
