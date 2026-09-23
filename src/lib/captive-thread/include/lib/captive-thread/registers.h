// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_REGISTERS_H_
#define SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_REGISTERS_H_

#include <zircon/assert.h>
#include <zircon/syscalls/debug.h>

#include <concepts>
#include <cstdint>
#include <optional>
#include <ranges>
#include <span>
#include <type_traits>

// This file provides various convenient accessors for the machine registers
// found in the zx_..._regs_t types.

namespace captive_thread {

// Each SomethingRegisters() function below takes an argument that can be a
// given zx_..._regs_t type, but of any value category: passed by value
// (rvalue); passed by const (lvalue) reference; passed by mutable (lvalue)
// reference.  This concept is used in those declarations to use a single
// template function rather than multiple nearly-identical overloads.
template <typename T, class Regs>
concept AnyValue = std::same_as<Regs, std::decay_t<T>>;

// libc++ is missing C++23 std::ranges::range_const_reference_t.
template <typename T>
using RangeConstReferenceType =  //
    std::common_reference_t<const std::ranges::range_value_t<T>&&,
                            std::ranges::range_reference_t<T>>;

// The functions below all return a RegisterRange.  This is a range of the
// individual registers in some order.  Each element of the range is a
// reference into the argument zx_..._regs_t object.  It's possible to pass a
// temporary (rvalue) into these functions, and they will return a range of
// references into that object.  So in these cases, the range must not be used
// past the lifetime of the argument object.  Usually the range objects are
// used in contexts where the temporary lifetime is extended to the end of the
// full expression.
template <typename T, typename R = uint64_t>
concept RegisterRange =  //
    std::ranges::bidirectional_range<T> && std::same_as<const R&, RangeConstReferenceType<T>> &&
    std::is_const_v<T> == std::is_const_v<std::ranges::range_reference_t<T>>;

// The SpecialRegisters() functions return a RegisterRange that gives the
// special registers in this order.  But the range object also supports these
// named accessors.  Not all machines have an ra() or scsp() register, so those
// accessors report std::nullopt instead (and are not included in the range).
template <typename T>
concept SpecialRegisterRange = RegisterRange<T> && requires(T regs) {
  { regs.pc() } -> std::common_reference_with<const uint64_t&&>;
  { regs.sp() } -> std::common_reference_with<const uint64_t&&>;
  { regs.fp() } -> std::common_reference_with<const uint64_t&&>;
  { regs.tp() } -> std::common_reference_with<const uint64_t&&>;
  { regs.ra() } -> std::convertible_to<std::optional<uint64_t>>;
  { regs.scsp() } -> std::convertible_to<std::optional<uint64_t>>;
};

// This adds the named accessors to the RegisterRange implementation that just
// yields the canonical order.
template <class Range>
class SpecialRegisterAdapter : public Range {
 public:
  constexpr SpecialRegisterAdapter() = default;

  constexpr explicit SpecialRegisterAdapter(Range range) : Range{std::move(range).base()} {}

  constexpr decltype(auto) pc(this auto&& self) { return Take<0>(self); }
  constexpr decltype(auto) sp(this auto&& self) { return Take<1>(self); }
  constexpr decltype(auto) fp(this auto&& self) { return Take<2>(self); }
  constexpr decltype(auto) tp(this auto&& self) { return Take<3>(self); }
  constexpr decltype(auto) ra(this auto&& self) { return Take<4>(self); }
  constexpr decltype(auto) scsp(this auto&& self) { return Take<5>(self); }

 private:
  using Array = std::decay_t<decltype(std::declval<Range>().base().base())>;
  template <typename T>
  static constexpr size_t kArrayOfSpanOfOneSize = 0;
  template <typename T, size_t N>
  static constexpr size_t kArrayOfSpanOfOneSize<std::array<std::span<T, 1>, N>> = N;
  static constexpr size_t kCount = kArrayOfSpanOfOneSize<Array>;
  static_assert(kCount >= 4);

  template <size_t I>
  static constexpr decltype(auto) Take(auto&& self) {
    if constexpr (I >= kCount) {
      return std::nullopt;
    } else {
      return *std::next(self.begin(), I);
    }
  }
};

// This is used to materialize a fixed-size span pointing into the struct.
template <size_t N, typename T>
constexpr auto AnyValueSpan(T* regs) {
  return std::span<T, N>{regs, N};
}

// This assembles a range from noncontiguous elements.
template <size_t N, size_t... M, typename T>
constexpr RegisterRange auto ScatterRange(std::span<T, N> first, std::span<T, M>... rest) {
  // std::views::join needs all the ranges to be the same type.  So if these
  // spans aren't all the same size, turn them into dynamic spans.
  using Dynamic = std::span<T>;
  if constexpr (((M == N) && ...)) {
    return std::views::join(std::array{first, rest...});
  }
  return std::views::join(std::array{Dynamic{first}, Dynamic{rest}...});
}

constexpr RegisterRange auto ScatterRange(AnyValue<uint64_t> auto&&... regs
                                          [[clang::lifetimebound]]) {
  return std::views::join(std::array{AnyValueSpan<1>(&regs)...});
}

// ArgumentRegisters() yields a range of the registers used to pass arguments.
constexpr RegisterRange auto ArgumentRegisters(
    AnyValue<zx_arm64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return std::span{regs.r}.template subspan<0, 8>();
}

// ReturnValueRegisters() yields a range of the registers used to return values
// from functions.
constexpr RegisterRange auto ReturnValueRegisters(
    AnyValue<zx_arm64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return std::span{regs.r}.template subspan<0, 2>();
}

// TemporaryRegisters() returns a range of the call-clobbered (callee-saves)
// registers that are not also argument registers.
constexpr RegisterRange auto TemporaryRegisters(
    AnyValue<zx_arm64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return std::span{regs.r}.template subspan<8, 10>();
}

// CallSavedRegisters() returns a range of the generic call-saved
// (caller-saves) registers.
constexpr RegisterRange auto CallSavedRegisters(
    AnyValue<zx_arm64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return std::span{regs.r}.template subspan<19, 10>();
}

constexpr SpecialRegisterRange auto SpecialRegisters(
    AnyValue<zx_arm64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return SpecialRegisterAdapter{
      ScatterRange(regs.pc, regs.sp, regs.r[29], regs.tpidr, regs.lr, regs.r[18])};
}

constexpr RegisterRange auto ArgumentRegisters(
    AnyValue<zx_riscv64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return AnyValueSpan<8>(&regs.a0);
}

constexpr RegisterRange auto ReturnValueRegisters(
    AnyValue<zx_riscv64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return AnyValueSpan<2>(&regs.a0);
}

constexpr RegisterRange auto TemporaryRegisters(
    AnyValue<zx_riscv64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(AnyValueSpan<3>(&regs.t0), AnyValueSpan<4>(&regs.t3));
}

constexpr RegisterRange auto CallSavedRegisters(
    AnyValue<zx_riscv64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(            // s0..s1 are discontiguous with s2..s11.
      AnyValueSpan<1>(&regs.s1),  // s0 is fp and so counted as "special".
      AnyValueSpan<10>(&regs.s2));
}

constexpr SpecialRegisterRange auto SpecialRegisters(
    AnyValue<zx_riscv64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return SpecialRegisterAdapter{ScatterRange(regs.pc, regs.sp, regs.s0, regs.tp, regs.ra, regs.gp)};
}

constexpr RegisterRange auto ArgumentRegisters(
    AnyValue<zx_x86_64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(regs.rdi, regs.rsi, regs.rdx, regs.rcx, regs.r8, regs.r9);
}

constexpr RegisterRange auto ReturnValueRegisters(
    AnyValue<zx_x86_64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(regs.rax, regs.rdx);
}

constexpr RegisterRange auto TemporaryRegisters(
    AnyValue<zx_x86_64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(regs.r10, regs.r11);
}

constexpr RegisterRange auto CallSavedRegisters(
    AnyValue<zx_x86_64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return ScatterRange(regs.rbx, regs.r12, regs.r13, regs.r14, regs.r15);
}

constexpr SpecialRegisterRange auto SpecialRegisters(
    AnyValue<zx_x86_64_thread_state_general_regs_t> auto&& regs [[clang::lifetimebound]]) {
  return SpecialRegisterAdapter{ScatterRange(regs.rip, regs.rsp, regs.rbp, regs.fs_base)};
}

}  // namespace captive_thread

#endif  // SRC_LIB_CAPTIVE_THREAD_INCLUDE_LIB_CAPTIVE_THREAD_REGISTERS_H_
