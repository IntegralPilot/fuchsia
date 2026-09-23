// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_STACK_ABI_H_
#define LIB_C_THREADS_STACK_ABI_H_

#include <lib/arch/asm.h>

#include <concepts>
#include <cstdint>
#include <type_traits>

#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Classes can use `[[no_unique_address]] IfShadowCallStack<T> member_;` along
// with `if constexpr (kShadowCallStackAbi)` guarding using `member_` as a T or
// separate overloads for NoStack and T.

// kShadowCallStackAbi indicates whether the Fuchsia Compiler ABI for this
// machine includes keeping the shadow-call-stack pointer register valid.
// Likewise, kSafeStackAbi indicates whether the Fuchsia Compiler ABI for this
// machine includes the SafeStack unsafe stack pointer being maintained in the
// $tp + ZX_TLS_UNSAFE_SP_OFFSET slot.  These are unchanging facts about the
// ABI for each machine.  Every build of libc is required to support the full
// ABI regardless of how libc itself is being compiled.
//
// The choice of compiler or configs used for the build determines whether all
// the normal libc code in a particular build itself _uses_ shadow-call-stack
// (and likewise the unsafe stack).  Bootstrapping realities mean that certain
// libc code (that built in the user.basic environment) itself _never_ uses the
// shadow-call-stack (or the unsafe stack)--including the code using this
// file's functions to bootstrap the shadow-call-stack ABI.  These questions of
// what libc's own code is _using_ have no bearing on the ABI mandate libc is
// _implementing_, which is specified here.
#if defined(__x86_64__)
inline constexpr bool kShadowCallStackAbi = false;
inline constexpr bool kSafeStackAbi = true;
#else
inline constexpr bool kShadowCallStackAbi = true;
inline constexpr bool kSafeStackAbi = false;
#endif

// An ABI element not supported has an empty object stand-in.
template <bool Enabled>
struct MaybeStackBase {};

// Distinct wrapper types for each constexpr variable ensure that two adjacent
// members will be of differing types so [[no_unique_address]] can take effect.
template <typename T, const bool& Enabled>
struct MaybeStack : MaybeStackBase<Enabled> {};

template <typename T, const bool& Enabled>
  requires(Enabled)
struct MaybeStack<T, Enabled> : MaybeStackBase<true> {
  constexpr MaybeStack() = default;
  constexpr MaybeStack(const MaybeStack&) = default;

  constexpr explicit MaybeStack(T x) : value{x} {}

  constexpr MaybeStack& operator=(const MaybeStack&) = default;
  constexpr MaybeStack& operator=(T new_value) {
    value = new_value;
    return *this;
  }

  T value{};
};

template <typename Stack>
concept NoStack = std::derived_from<std::decay_t<Stack>, MaybeStackBase<false>>;

template <typename Stack>
concept SomeStack = std::derived_from<std::decay_t<Stack>, MaybeStackBase<true>>;

template <typename T>
using IfShadowCallStack = MaybeStack<T, kShadowCallStackAbi>;

template <typename T>
using IfSafeStack = MaybeStack<T, kSafeStackAbi>;

// This is unconditional but using the wrapper keeps things uniform.
template <typename T>
using MachineStack = MaybeStack<T, std::true_type::value>;

template <SomeStack T>
inline constexpr bool kStackGrowsUp = false;

template <typename T>
inline constexpr bool kStackGrowsUp<MaybeStack<T, kShadowCallStackAbi>> = true;

constexpr void OnStack(NoStack auto&&, auto&& f) {}
constexpr void OnStack(SomeStack auto&& stack, std::invocable<decltype(stack.value)> auto&& f) {
  std::forward<decltype(f)>(f)(stack.value);
}

constexpr auto StackOr(NoStack auto&&, auto value) { return value; }
constexpr auto StackOr(SomeStack auto&& stack,
                       std::convertible_to<decltype(stack.value)> auto value) {
  return stack.value;
}

// This function is only used in code compiled for the basic machine ABI and
// only if kShadowCallStackAbi is true.  It installs the shadow call stack.
#if !__has_feature(shadow_call_stack)
inline void ShadowCallStackSet(uint64_t* scsp) {
#if defined(__aarch64__)
  __asm__ volatile("mov x18, %0" : : "r"(scsp));
  return;
#elif defined(__riscv)
  __asm__ volatile("mv gp, %0" : : "r"(scsp));
  return;
#endif
  __builtin_abort();
}
#endif  //  !__has_feature(shadow_call_stack)

// This must be called first thing in the first function that runs with the
// full compiler ABI available.  In builds of libc without shadow-call-stack
// support enabled on machines where the ABI includes it, this mimics what the
// compiler's (non-leaf) function prologue would usually do.  This ensures that
// however libc is built, the shadow-call-stack backtraces are consistent with
// the frame-pointer backtraces for the initial frames, yielding a predictable
// backtrace of _start -> __libc_start_main -> main via CFI, frame-pointer, and
// shadow-call-stack techniques.  If main and the code it calls (outside libc)
// do use shadow-call-stack and expect good backtraces taken purely from the
// shadow call stack, then the outermost frames will match expectations.
[[gnu::always_inline]] inline void ShadowCallStackPrologue(
    // This is a bit of belt-and-suspenders.  The always_inline attribute by
    // itself should ensure this is inlined into __libc_start_main and so
    // __builtin_return_address(0) used in the body would be evaluated as if in
    // the caller anyway.  But a default argument is always formally evaluated
    // in the caller's context, so that also guarantees it (and technically
    // makes it unnecessary to ensure this gets inlined, though it's only one
    // or two instructions and so obviously should be!).
    void* caller = __builtin_return_address(0)) {
#ifndef __x86_64__
  static_assert(kShadowCallStackAbi);

#if !__has_feature(shadow_call_stack)

  // The INIT asm template pushes our own return address on the shadow call
  // stack so it appears in a backtrace just as it would if this function
  // itself were using the normal shadow-call-stack protocol.  Before that, it
  // pushes a zero return address as an end marker similar to how CFI unwinding
  // marks the base frame by having its return address column compute zero and
  // FP unwinding marks the base frame by having its prior FP value be zero.
  // The kDwarfRegno identifies the ABI's shadow-call-stack pointer register,
  // so CFI can describe how to get the caller's value.
#if defined(__aarch64__)
  constexpr int kDwarfRegno = 18;
#define LIBC_SHADOW_CALL_STACK_INIT(cfi_asm_template) \
  /* One instruction does it all.  */                 \
  "stp xzr, %[ra], [x18], #16\n" cfi_asm_template
#elif defined(__riscv)
  constexpr int kDwarfRegno = 3;
#define LIBC_SHADOW_CALL_STACK_INIT(cfi_asm_template)                     \
  /* The first instruction moves the pointer so the CFI is necessary.  */ \
  "add gp, gp, 16\n" cfi_asm_template                                     \
  "sd zero, -16(gp)\n"                                                    \
  "sd %[ra], -8(gp)\n"
#endif

  __asm__ volatile(  // This uses %[ra] as an input operand.
      LIBC_SHADOW_CALL_STACK_INIT(
          // DW_CFA_val_expression <regno>, { DW_OP_breg<regno> -16 }
          ".cfi_escape %c[insn], %c[regno], 2, %c[breg], (-16 & 0x7f)")
      :
      : [insn] "i"(DW_CFA_val_expression), [ra] "r"(caller),  //
        [regno] "i"(kDwarfRegno), [breg] "i"(DW_OP_breg(kDwarfRegno)));
  static_assert(kDwarfRegno < 32, "needs DW_OP_bregx, maybe ULEB128");
#undef LIBC_SHADOW_CALL_STACK_INIT

#endif  // !__has_feature(shadow_call_stack)
#endif  // !__x86_64__
}

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_STACK_ABI_H_
