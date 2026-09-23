// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fasttime/time.h>
#include <zircon/compiler.h>

extern "C" {
__EXPORT uint64_t loadable_check_fasttime_version(const fasttime::internal::TimeValues* values) {
  return static_cast<uint64_t>(check_fasttime_version(*values));
}

__EXPORT zx_time_t loadable_compute_monotonic_time(const fasttime::internal::TimeValues* values) {
  return compute_monotonic_time(*values);
}

__EXPORT zx_ticks_t loadable_compute_monotonic_ticks(const fasttime::internal::TimeValues* values) {
  return compute_monotonic_ticks(*values);
}

__EXPORT zx_time_t
loadable_fasttime_compute_monotonic_time(const fasttime::internal::TimeValues* values) {
  return fasttime::compute_monotonic_time(reinterpret_cast<zx_vaddr_t>(values));
}

__EXPORT zx_ticks_t
loadable_fasttime_compute_monotonic_ticks(const fasttime::internal::TimeValues* values) {
  return fasttime::compute_monotonic_ticks(reinterpret_cast<zx_vaddr_t>(values));
}

__EXPORT zx_time_t
loadable_compute_monotonic_time_skip_validation(const fasttime::internal::TimeValues* values) {
  return fasttime::internal::compute_monotonic_time<
      fasttime::internal::FasttimeVerificationMode::kSkip>(*values);
}

__EXPORT zx_ticks_t
loadable_compute_monotonic_ticks_skip_validation(const fasttime::internal::TimeValues* values) {
  return fasttime::internal::compute_monotonic_ticks<
      fasttime::internal::FasttimeVerificationMode::kSkip>(*values);
}

}  // extern "C"
