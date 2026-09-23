// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_INTERNAL_ATOMIC_H_
#define LIB_STDCOMPAT_INTERNAL_ATOMIC_H_

#include <algorithm>
#include <atomic>
#include <cstddef>
#include <type_traits>

#include "../memory.h"
#include "../type_traits.h"
#include "linkage.h"

namespace cpp20 {
namespace atomic_internal {

// Maps |std::memory_order| to builtin |__ATOMIC_XXXX| values.
LIB_STDCOMPAT_INLINE_LINKAGE constexpr int to_builtin_memory_order(std::memory_order order) {
  switch (order) {
    case std::memory_order_relaxed:
      return __ATOMIC_RELAXED;
    case std::memory_order_consume:
      return __ATOMIC_CONSUME;
    case std::memory_order_acquire:
      return __ATOMIC_ACQUIRE;
    case std::memory_order_release:
      return __ATOMIC_RELEASE;
    case std::memory_order_acq_rel:
      return __ATOMIC_ACQ_REL;
    case std::memory_order_seq_cst:
      return __ATOMIC_SEQ_CST;
    default:
      __builtin_abort();
  }
}

// Applies corrections to |order| on |compare_exchange|'s load operations.
LIB_STDCOMPAT_INLINE_LINKAGE constexpr std::memory_order compare_exchange_load_memory_order(
    std::memory_order order) {
  if (order == std::memory_order_acq_rel) {
    return std::memory_order_acquire;
  }

  if (order == std::memory_order_release) {
    return std::memory_order_relaxed;
  }

  return order;
}

// Unspecialized types alignment, who have a size that matches a known integer(power of 2 bytes)
// should be aligned to its size at least for better performance. Otherwise, we default to its
// default alignment.
template <typename T, typename Enabled = void>
struct alignment {
  static constexpr size_t required_alignment =
      std::max((sizeof(T) & (sizeof(T) - 1) || sizeof(T) > 16) ? 0 : sizeof(T), alignof(T));
};

// Remove volatile from parameter and defer template instantiation.
template <typename T>
using value_t = std::remove_volatile_t<T>;

template <typename T>
struct alignment<T, std::enable_if_t<std::is_integral_v<T>>> {
  static constexpr size_t required_alignment = sizeof(T) > alignof(T) ? sizeof(T) : alignof(T);
};

template <typename T>
struct alignment<T, std::enable_if_t<std::is_pointer_v<T> || std::is_floating_point_v<T>>> {
  static constexpr size_t required_alignment = alignof(T);
};

template <typename T>
static constexpr bool unqualified = std::is_same_v<T, std::remove_cv_t<T>>;

template <typename T>
LIB_STDCOMPAT_INLINE_LINKAGE inline bool compare_exchange(T* ptr, value_t<T>& expected,
                                                          value_t<T> desired, bool is_weak,
                                                          std::memory_order success,
                                                          std::memory_order failure) {
  return __atomic_compare_exchange(ptr, std::addressof(expected), std::addressof(desired), is_weak,
                                   to_builtin_memory_order(success),
                                   to_builtin_memory_order(failure));
}

// TODO(https://github.com/llvm/llvm-project/issues/94879): Clean up when atomic_ref<T> interface
// is cleaned up.
#pragma GCC diagnostic push
#if defined(__clang__)
#pragma GCC diagnostic ignored "-Wdeprecated-volatile"
#elif defined(__GNUG__)
#pragma GCC diagnostic ignored "-Wvolatile"
#endif

// Provide atomic operations based on compiler builtins.
template <typename Derived, typename T>
class atomic_ops {
 public:
  LIB_STDCOMPAT_INLINE_LINKAGE void store(
      T desired, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    __atomic_store(ptr(), std::addressof(desired), to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T
  load(std::memory_order order = std::memory_order_seq_cst) const noexcept {
    alignas(T) std::byte store[sizeof(T)];
    auto* ret = reinterpret_cast<std::remove_const_t<value_t<T>>*>(&store);
    __atomic_load(ptr(), ret, to_builtin_memory_order(order));
    return *ret;
  }

  LIB_STDCOMPAT_INLINE_LINKAGE operator T() const noexcept { return this->load(); }

  LIB_STDCOMPAT_INLINE_LINKAGE T
  exchange(T desired, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    alignas(T) std::byte store[sizeof(T)];
    value_t<T> nv_desired = desired;
    auto* ret = reinterpret_cast<value_t<T>*>(&store);
    __atomic_exchange(ptr(), std::addressof(nv_desired), ret, to_builtin_memory_order(order));
    return *ret;
  }

  LIB_STDCOMPAT_INLINE_LINKAGE bool compare_exchange_weak(
      T& expected, T desired,
      std::memory_order success = std::memory_order_seq_cst) const noexcept {
    return compare_exchange_weak(expected, desired, success,
                                 compare_exchange_load_memory_order(success));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE bool compare_exchange_weak(
      T& expected, T desired, std::memory_order success, std::memory_order failure) const noexcept {
    check_failure_memory_order(failure);
    return compare_exchange(ptr(), expected, desired,
                            /*is_weak=*/true, success, failure);
  }

  LIB_STDCOMPAT_INLINE_LINKAGE bool compare_exchange_strong(
      T& expected, T desired,
      std::memory_order success = std::memory_order_seq_cst) const noexcept {
    return compare_exchange_strong(expected, desired, success,
                                   compare_exchange_load_memory_order(success));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE bool compare_exchange_strong(
      T& expected, T desired, std::memory_order success, std::memory_order failure) const noexcept {
    check_failure_memory_order(failure);
    return compare_exchange(ptr(), expected, desired,
                            /*is_weak=*/false, success, failure);
  }

 private:
  // |failure| memory order may not be |std::memory_order_release| or
  // |std::memory_order_acq_release|.
  LIB_STDCOMPAT_INLINE_LINKAGE constexpr void check_failure_memory_order(
      std::memory_order failure) const {
    if (failure == std::memory_order_acq_rel || failure == std::memory_order_release) {
      __builtin_abort();
    }
  }
  constexpr T* ptr() const { return static_cast<const Derived*>(this)->ptr_; }
};

// Delegate to helper templates the arguments which differ between pointer and integral types.
template <typename T>
struct arithmetic_ops_helper {
  // Return type of |ptr| method.
  using ptr_type = T*;

  // Return type of atomic builtins.
  using return_type = std::remove_volatile_t<T>;

  // Type of operands used.
  using operand_type = std::remove_volatile_t<T>;

  // Arithmetic operands are amplified by this scalar.
  static constexpr size_t modifier = 1;
};

template <typename T>
struct arithmetic_ops_helper<T*> {
  // Return type of |ptr| method.
  using ptr_type = T**;

  // Return type of atomic builtins.
  using return_type = T*;

  // Type of operands used.
  using operand_type = ptrdiff_t;

  // Arithmetic operands are amplified by this scalar.
  static constexpr size_t modifier = sizeof(T);
};

template <typename T>
constexpr bool is_numeric_v =
    !std::is_same_v<T, bool> && (std::is_integral_v<T> || std::is_floating_point_v<T>);

// difference_t is only defined for pointers, integral types and floating types.
template <typename T, typename = void>
struct atomic_difference_type {};

template <typename T>
struct atomic_difference_type<T, std::enable_if_t<std::is_pointer_v<T>>> {
  using difference_type = std::ptrdiff_t;
};

template <typename T>
struct atomic_difference_type<T, std::enable_if_t<is_numeric_v<T>>> {
  using difference_type = T;
};

// Arithmetic operations.
//
// Enables :
//  - fetch_add
//  - fetch_sub
//  - operator++
//  - operator--
//  - operator+=
//  - operator-=
template <typename Derived, typename T, typename Enabled = void>
class arithmetic_ops {};

// Non volatile pointers Pointer and Integral operations.
template <typename Derived, typename T>
class arithmetic_ops<
    Derived, T,
    std::enable_if_t<std::is_integral_v<T> || (std::is_pointer_v<T> && !std::is_volatile_v<T>)>> {
  using return_t = typename arithmetic_ops_helper<T>::return_type;
  using operand_t = typename arithmetic_ops_helper<T>::operand_type;
  using ptr_t = typename arithmetic_ops_helper<T>::ptr_type;
  static constexpr auto modifier = arithmetic_ops_helper<T>::modifier;

 public:
  LIB_STDCOMPAT_INLINE_LINKAGE return_t
  fetch_add(operand_t operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    return __atomic_fetch_add(ptr(), operand * modifier, to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE return_t
  fetch_sub(operand_t operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    return __atomic_fetch_sub(ptr(), operand * modifier, to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator++(int) const noexcept { return fetch_add(1); }
  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator--(int) const noexcept { return fetch_sub(1); }
  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator++() const noexcept { return fetch_add(1) + 1; }
  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator--() const noexcept { return fetch_sub(1) - 1; }
  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator+=(operand_t operand) const noexcept {
    return fetch_add(operand) + operand;
  }
  LIB_STDCOMPAT_INLINE_LINKAGE return_t operator-=(operand_t operand) const noexcept {
    return fetch_sub(operand) - operand;
  }

 private:
  LIB_STDCOMPAT_INLINE_LINKAGE constexpr ptr_t ptr() const {
    return static_cast<const Derived*>(this)->ptr_;
  }
};

// Floating point arithmetic operations.
// Based on CAS cycles to perform atomic add and sub.
template <typename Derived, typename T>
class arithmetic_ops<Derived, T, std::enable_if_t<std::is_floating_point_v<T>>> {
 public:
  LIB_STDCOMPAT_INLINE_LINKAGE T
  fetch_add(T operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    value_t<T> old_value = derived()->load(std::memory_order_relaxed);
    value_t<T> new_value = old_value + operand;
    while (
        !compare_exchange(ptr(), old_value, new_value, false, order, std::memory_order_relaxed)) {
      new_value = old_value + operand;
    }
    return old_value;
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T
  fetch_sub(T operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    value_t<T> old_value = derived()->load(std::memory_order_relaxed);
    value_t<T> new_value = old_value - operand;
    while (
        !compare_exchange(ptr(), old_value, new_value, false, order, std::memory_order_relaxed)) {
      new_value = old_value - operand;
    }
    return old_value;
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T operator+=(T operand) const noexcept {
    return fetch_add(operand) + operand;
  }
  LIB_STDCOMPAT_INLINE_LINKAGE T operator-=(T operand) const noexcept {
    return fetch_sub(operand) - operand;
  }

 private:
  LIB_STDCOMPAT_INLINE_LINKAGE constexpr T* ptr() const {
    return static_cast<const Derived*>(this)->ptr_;
  }
  LIB_STDCOMPAT_INLINE_LINKAGE constexpr const Derived* derived() const {
    return static_cast<const Derived*>(this);
  }
};

// Bitwise operations.
//
// Enables :
//  - fetch_and
//  - fetch_or
//  - fetch_xor
//  - operator&=
//  - operator|=
//  - operator^=
template <typename Derived, typename T, typename Enabled = void>
class bitwise_ops {};

template <typename Derived, typename T>
class bitwise_ops<Derived, T, std::enable_if_t<std::is_integral_v<T>>> {
 public:
  LIB_STDCOMPAT_INLINE_LINKAGE T
  fetch_and(T operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    return __atomic_fetch_and(ptr(), static_cast<value_t<T>>(operand),
                              to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T
  fetch_or(T operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    return __atomic_fetch_or(ptr(), static_cast<value_t<T>>(operand),
                             to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T
  fetch_xor(T operand, std::memory_order order = std::memory_order_seq_cst) const noexcept {
    return __atomic_fetch_xor(ptr(), static_cast<value_t<T>>(operand),
                              to_builtin_memory_order(order));
  }

  LIB_STDCOMPAT_INLINE_LINKAGE T operator&=(T operand) const noexcept {
    return fetch_and(operand) & static_cast<value_t<T>>(operand);
  }
  LIB_STDCOMPAT_INLINE_LINKAGE T operator|=(T operand) const noexcept {
    return fetch_or(operand) | static_cast<value_t<T>>(operand);
  }
  LIB_STDCOMPAT_INLINE_LINKAGE T operator^=(T operand) const noexcept {
    return fetch_xor(operand) ^ static_cast<value_t<T>>(operand);
  }

 private:
  LIB_STDCOMPAT_INLINE_LINKAGE constexpr T* ptr() const {
    return static_cast<const Derived*>(this)->ptr_;
  }
};

// TODO(https://github.com/llvm/llvm-project/issues/94879): Clean up when atomic_ref<T> interface
// is cleaned up.
#pragma GCC diagnostic pop

}  // namespace atomic_internal
}  // namespace cpp20

#endif  // LIB_STDCOMPAT_INTERNAL_ATOMIC_H_
