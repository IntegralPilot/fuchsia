// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vdso_common.h"

#include <errno.h>
#include <lib/fasttime/time.h>
#include <sys/syscall.h>

#include "vdso_calculate_time.h"
#include "vdso_platform.h"

zx_instant_mono_ticks_t calculate_monotonic_ticks() {
  zx_vaddr_t time_values_addr = reinterpret_cast<zx_vaddr_t>(&time_values);
  return fasttime::compute_monotonic_ticks(time_values_addr);
}

zx_instant_boot_ticks_t calculate_boot_ticks() {
  zx_vaddr_t time_values_addr = reinterpret_cast<zx_vaddr_t>(&time_values);
  return fasttime::compute_boot_ticks(time_values_addr);
}

zx_instant_mono_t calculate_monotonic_time_nsec() {
  zx_vaddr_t time_values_addr = reinterpret_cast<zx_vaddr_t>(&time_values);
  return fasttime::compute_monotonic_time(time_values_addr);
}

zx_instant_boot_t calculate_boot_time_nsec() {
  zx_vaddr_t time_values_addr = reinterpret_cast<zx_vaddr_t>(&time_values);
  return fasttime::compute_boot_time(time_values_addr);
}

static void to_nanoseconds(uint64_t time_in_ns, time_t* tv_sec, long int* tv_nsec) {
  uint64_t tv_sec64 = time_in_ns / kNanosecondsPerSecond;
  if (tv_sec64 > INT_MAX) {
    *tv_sec = INT_MAX;
  } else {
    *tv_sec = static_cast<time_t>(tv_sec64);
  }
  uint64_t tv_nsec64 = time_in_ns % kNanosecondsPerSecond;
  if (tv_nsec64 > LONG_MAX) {
    *tv_nsec = LONG_MAX;
  } else {
    *tv_nsec = static_cast<long int>(tv_nsec64);
  }
}

int clock_gettime_impl(int clock_id, timespec* tp) {
  int ret = 0;
  if ((clock_id == CLOCK_MONOTONIC) || (clock_id == CLOCK_MONOTONIC_RAW) ||
      (clock_id == CLOCK_MONOTONIC_COARSE)) {
    int64_t monot_nsec = calculate_monotonic_time_nsec();
    if (monot_nsec == ZX_TIME_INFINITE_PAST) {
      // If calculate_monotonic_time_nsec returned ZX_TIME_INFINITE_PAST, then either:
      // 1. The ticks register is not userspace accessible, or
      // 2. The version of libfasttime is out of sync with the version of the time values VMO.
      // In either case, we must invoke a syscall to get the correct time value.
      ret = syscall(__NR_clock_gettime, static_cast<intptr_t>(clock_id),
                    reinterpret_cast<intptr_t>(tp), 0);
      return ret;
    }
    to_nanoseconds(static_cast<uint64_t>(monot_nsec), &tp->tv_sec, &tp->tv_nsec);
  } else if (clock_id == CLOCK_BOOTTIME) {
    int64_t boot_nsec = calculate_boot_time_nsec();
    if (boot_nsec == ZX_TIME_INFINITE_PAST) {
      // Once again, if calculate_boot_time_nsec returned ZX_TIME_INFINITE_PAST, then either:
      // 1. The ticks register is not userspace accessible, or
      // 2. The version of libfasttime is out of sync with the version of the time values VMO.
      // In either case, we must invoke a syscall to get the correct time value.
      ret = syscall(__NR_clock_gettime, static_cast<intptr_t>(clock_id),
                    reinterpret_cast<intptr_t>(tp), 0);
      return ret;
    }
    to_nanoseconds(static_cast<uint64_t>(boot_nsec), &tp->tv_sec, &tp->tv_nsec);
  } else if (clock_id == CLOCK_REALTIME) {
    int64_t utc_nsec = calculate_utc_time_nsec();
    if (utc_nsec == kUtcInvalid) {
      // The syscall is used instead of endlessly retrying to acquire the seqlock. This gives the
      // writer thread of the seqlock a chance to run, even if it happens to have a lower priority
      // than the current thread.
      ret = syscall(__NR_clock_gettime, static_cast<intptr_t>(clock_id),
                    reinterpret_cast<intptr_t>(tp), 0);
      return ret;
    }
    to_nanoseconds(static_cast<uint64_t>(utc_nsec), &tp->tv_sec, &tp->tv_nsec);
  } else {
    ret = syscall(__NR_clock_gettime, static_cast<intptr_t>(clock_id),
                  reinterpret_cast<intptr_t>(tp), 0);
  }
  return ret;
}

bool is_valid_cpu_clock(int clock_id) { return (clock_id & 7) != 7 && (clock_id & 3) < 3; }

int clock_getres_impl(int clock_id, timespec* tp) {
  if (clock_id < 0 && !is_valid_cpu_clock(clock_id)) {
    return -EINVAL;
  }
  if (tp == nullptr) {
    return 0;
  }
  switch (clock_id) {
    case CLOCK_REALTIME:
    case CLOCK_REALTIME_ALARM:
    case CLOCK_REALTIME_COARSE:
    case CLOCK_MONOTONIC:
    case CLOCK_MONOTONIC_COARSE:
    case CLOCK_MONOTONIC_RAW:
    case CLOCK_BOOTTIME:
    case CLOCK_BOOTTIME_ALARM:
    case CLOCK_THREAD_CPUTIME_ID:
    case CLOCK_PROCESS_CPUTIME_ID:
      tp->tv_sec = 0;
      tp->tv_nsec = 1;
      return 0;

    default:
      int ret = syscall(__NR_clock_getres, static_cast<intptr_t>(clock_id),
                        reinterpret_cast<intptr_t>(tp), 0);
      return ret;
  }
}

int gettimeofday_impl(timeval* tv, struct timezone* tz) {
  if (tz != nullptr) {
    int ret = syscall(__NR_gettimeofday, reinterpret_cast<intptr_t>(tv),
                      reinterpret_cast<intptr_t>(tz), 0);
    return ret;
  }
  if (tv == nullptr) {
    return 0;
  }
  int64_t utc_nsec = calculate_utc_time_nsec();
  if (utc_nsec != kUtcInvalid) {
    // TODO(https://fxbug.dev/380431929): Are we going to lose any necessary precision?
    to_nanoseconds(utc_nsec, &tv->tv_sec, &tv->tv_usec);
    tv->tv_usec = static_cast<long>(static_cast<unsigned long>(tv->tv_usec) / 1'000);
    return 0;
  }

  // The syscall is used instead of endlessly retrying to acquire the seqlock. This gives the
  // writer thread of the seqlock a chance to run, even if it happens to have a lower priority
  // than the current thread.
  int ret =
      syscall(__NR_gettimeofday, reinterpret_cast<intptr_t>(tv), reinterpret_cast<intptr_t>(tz), 0);
  return ret;
}
