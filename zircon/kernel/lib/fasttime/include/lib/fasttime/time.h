// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_TIME_H_
#define ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_TIME_H_

#include <lib/fasttime/internal/abi.h>
#include <lib/fasttime/internal/time.h>
#include <zircon/time.h>
#include <zircon/types.h>

namespace fasttime {

inline zx_instant_mono_ticks_t compute_monotonic_ticks(zx_vaddr_t time_values_addr) {
  const internal::TimeValues* time_values =
      reinterpret_cast<internal::TimeValues*>(time_values_addr);
  return internal::compute_monotonic_ticks<internal::FasttimeVerificationMode::kNormal>(
      *time_values);
}

inline zx_instant_mono_t compute_monotonic_time(zx_vaddr_t time_values_addr) {
  const internal::TimeValues* time_values =
      reinterpret_cast<internal::TimeValues*>(time_values_addr);
  return internal::compute_monotonic_time<internal::FasttimeVerificationMode::kNormal>(
      *time_values);
}

inline zx_instant_boot_ticks_t compute_boot_ticks(zx_vaddr_t time_values_addr) {
  const internal::TimeValues* time_values =
      reinterpret_cast<internal::TimeValues*>(time_values_addr);
  return internal::compute_boot_ticks<internal::FasttimeVerificationMode::kNormal>(*time_values);
}

inline zx_instant_boot_t compute_boot_time(zx_vaddr_t time_values_addr) {
  const internal::TimeValues* time_values =
      reinterpret_cast<internal::TimeValues*>(time_values_addr);
  return internal::compute_boot_time<internal::FasttimeVerificationMode::kNormal>(*time_values);
}

}  // namespace fasttime

#endif  // ZIRCON_KERNEL_LIB_FASTTIME_INCLUDE_LIB_FASTTIME_TIME_H_
