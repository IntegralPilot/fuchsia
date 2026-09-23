// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANDROID_CORE_INCLUDE_CUTILS_ATOMIC_H_
#define SRC_LIB_ANDROID_CORE_INCLUDE_CUTILS_ATOMIC_H_

#include <atomic>

static inline volatile std::atomic_int_least32_t* to_atomic_int_least32_t(
    volatile const int32_t* addr) {
  return reinterpret_cast<volatile std::atomic_int_least32_t*>(const_cast<volatile int32_t*>(addr));
}

static inline int32_t android_atomic_add(int32_t value, volatile int32_t* addr) {
  volatile std::atomic_int_least32_t* a = to_atomic_int_least32_t(addr);
  return std::atomic_fetch_add_explicit(a, value, std::memory_order_release);
}

static inline int32_t android_atomic_inc(volatile int32_t* addr) {
  volatile std::atomic_int_least32_t* a = to_atomic_int_least32_t(addr);
  return std::atomic_fetch_add_explicit(a, 1, std::memory_order_release);
}

#endif  // SRC_LIB_ANDROID_CORE_INCLUDE_CUTILS_ATOMIC_H_
