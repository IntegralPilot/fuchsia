// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <sys/time.h>
#include <zircon/compiler.h>

#include "vdso_calculate_time.h"
#include "vdso_common.h"
#include "vdso_platform.h"

int syscall(intptr_t syscall_number, intptr_t arg1, intptr_t arg2, intptr_t arg3) {
  register intptr_t x0 asm("r0") = arg1;
  register intptr_t x1 asm("r1") = arg2;
  register intptr_t x2 asm("r2") = arg3;
  register intptr_t number asm("r7") = syscall_number;

  __asm__ volatile("svc #0" : "=r"(x0) : "0"(x0), "r"(x1), "r"(x2), "r"(number) : "memory");
  return static_cast<int>(x0);
}

extern "C" __EXPORT __attribute__((naked)) __attribute__((target("arm"))) void
__kernel_rt_sigreturn() {
  __asm__ volatile("mov r7, %0" ::"I"(__NR_rt_sigreturn));
  __asm__ volatile("svc #0");
}

extern "C" __EXPORT int __vdso_clock_gettime(int clock_id, struct timespec* tp) {
  return clock_gettime_impl(clock_id, tp);
}

extern "C" __EXPORT int __vdso_gettimeofday(struct timeval* tv, struct timezone* tz) {
  return gettimeofday_impl(tv, tz);
}
