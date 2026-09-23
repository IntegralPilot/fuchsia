// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <sys/time.h>
#include <zircon/compiler.h>

#include "src/starnix/kernel/vdso/vdso_common.h"
#include "src/starnix/kernel/vdso/vdso_platform.h"

int syscall(intptr_t syscall_number, intptr_t arg1, intptr_t arg2, intptr_t arg3) {
  int ret;
  __asm__ volatile("syscall;"
                   : "=a"(ret)
                   : "a"(syscall_number), "D"(arg1), "S"(arg2), "d"(arg3)
                   : "rcx", "r11", "memory");
  return ret;
}

extern "C" __EXPORT int __vdso_clock_gettime(int clock_id, struct timespec* tp) {
  return clock_gettime_impl(clock_id, tp);
}

extern "C" __EXPORT __WEAK_ALIAS("__vdso_clock_gettime") int clock_gettime(int clock_id,
                                                                           struct timespec* tp);

extern "C" __EXPORT int __vdso_clock_getres(int clock_id, struct timespec* tp) {
  return clock_getres_impl(clock_id, tp);
}

extern "C" __EXPORT __WEAK_ALIAS("__vdso_clock_getres") int clock_getres(int clock_id,
                                                                         struct timespec* tp);

extern "C" __EXPORT int __vdso_getcpu(unsigned int* cpu, unsigned int* cache) {
  int ret =
      syscall(__NR_getcpu, reinterpret_cast<intptr_t>(cpu), reinterpret_cast<intptr_t>(cache), 0);
  return ret;
}

extern "C" __EXPORT __WEAK_ALIAS("__vdso_getcpu") int getcpu(unsigned int* cpu,
                                                             unsigned int* cache);

extern "C" __EXPORT int __vdso_gettimeofday(struct timeval* tv, struct timezone* tz) {
  return gettimeofday_impl(tv, tz);
}

extern "C" __EXPORT __WEAK_ALIAS("__vdso_gettimeofday") int gettimeofday(struct timeval* tv,
                                                                         struct timezone* tz);

extern "C" __EXPORT time_t __vdso_time(time_t* t) {
  int ret = syscall(__NR_time, reinterpret_cast<intptr_t>(t), 0, 0);
  return ret;
}

extern "C" __EXPORT __WEAK_ALIAS("__vdso_time") time_t time(time_t* t);
