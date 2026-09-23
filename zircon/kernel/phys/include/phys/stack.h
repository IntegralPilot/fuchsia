// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_STACK_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_STACK_H_

#include <zircon/compiler.h>

#define BOOT_STACK_ALIGN 16
#define BOOT_STACK_SIZE 16384

#if __has_feature(shadow_call_stack)
#define BOOT_SHADOW_CALL_STACK_SIZE 512
#else
#define BOOT_SHADOW_CALL_STACK_SIZE 0
#endif  // __has_feature(shadow_call_stack)

#ifndef __ASSEMBLER__
#include <lib/arch/backtrace.h>

#include <ktl/iterator.h>

struct BootStack {
  // Returns true iff SP falls on this stack.
  // This considers the limit to be "on".
  bool IsOnStack(uintptr_t sp) const;

  uintptr_t InitialSp() const { return reinterpret_cast<uintptr_t>(ktl::end(stack)); }

  alignas(BOOT_STACK_ALIGN) uint8_t stack[BOOT_STACK_SIZE];
  static_assert(BOOT_STACK_SIZE % BOOT_STACK_ALIGN == 0);
};
static_assert(alignof(BootStack) == BOOT_STACK_ALIGN);

struct NoStack {
  static constexpr bool kEnabled = false;

  bool IsOnStack(uintptr_t sp) const { return false; }

  constexpr uintptr_t InitialSp() const { return 0; }

  arch::ShadowCallStackBacktrace BackTrace(uintptr_t scsp = 0) const { return {}; }
};

#if __has_feature(safe_stack)
using BootUnsafeStack = BootStack;
#else
using BootUnsafeStack = NoStack;
#endif  // __has_feature(safe_stack)

#if __has_feature(shadow_call_stack)
struct BootShadowCallStack {
  static constexpr bool kEnabled = true;

  uintptr_t StartScsp() const { return reinterpret_cast<uintptr_t>(ktl::end(shadow_call_stack)); }

  // The caller evaluates the default argument to supply its own backtrace.
  // That way the immediate caller itself is not included in the backtrace.
  arch::ShadowCallStackBacktrace BackTrace(
      uintptr_t scsp = arch::GetShadowCallStackPointer()) const;

  uintptr_t shadow_call_stack[BOOT_SHADOW_CALL_STACK_SIZE / sizeof(uintptr_t)];
};
static_assert(alignof(BootShadowCallStack) == 8);
static_assert(BOOT_SHADOW_CALL_STACK_SIZE % sizeof(uintptr_t) == 0);

#else

using BootShadowCallStack = NoStack;

#endif  // __has_feature(shadow_call_stack)

extern BootStack boot_stack, phys_exception_stack;
extern BootUnsafeStack boot_unsafe_stack, phys_exception_unsafe_stack;
extern BootShadowCallStack boot_shadow_call_stack, phys_exception_shadow_call_stack;

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_STACK_H_
