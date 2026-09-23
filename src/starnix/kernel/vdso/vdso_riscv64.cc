// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <sys/time.h>
#include <zircon/compiler.h>

#include "vdso_common.h"
#include "vdso_platform.h"

int syscall(intptr_t syscall_number, intptr_t arg1, intptr_t arg2, intptr_t arg3) {
  register intptr_t a0 asm("a0") = arg1;
  register intptr_t a1 asm("a1") = arg2;
  register intptr_t a2 asm("a2") = arg3;
  register intptr_t number asm("a7") = syscall_number;

  __asm__ volatile("ecall" : "=r"(a0) : "0"(a0), "r"(a1), "r"(a2), "r"(number) : "memory");
  return static_cast<int>(a0);
}

extern "C" __EXPORT __attribute__((naked)) void __vdso_rt_sigreturn() {
  __asm__ volatile("li a7, %0" ::"I"(__NR_rt_sigreturn));
  __asm__ volatile("ecall");
}

extern "C" __EXPORT int __vdso_clock_gettime(int clock_id, struct timespec* tp) {
  return syscall(__NR_clock_gettime, static_cast<intptr_t>(clock_id),
                 reinterpret_cast<intptr_t>(tp), 0);
}

extern "C" __EXPORT int __vdso_clock_getres(int clock_id, struct timespec* tp) {
  return clock_getres_impl(clock_id, tp);
}

extern "C" __EXPORT int __vdso_gettimeofday(struct timeval* tv, struct timezone* tz) {
  return syscall(__NR_gettimeofday, reinterpret_cast<intptr_t>(tv), reinterpret_cast<intptr_t>(tz),
                 0);
}

extern "C" __EXPORT int __vdso_getcpu(unsigned int* cpu, unsigned int* node, void* unused) {
  return syscall(__NR_getcpu, reinterpret_cast<intptr_t>(cpu), reinterpret_cast<intptr_t>(node),
                 reinterpret_cast<intptr_t>(unused));
}

extern "C" __EXPORT int __vdso_flush_icache(void* start, void* end, unsigned long flags) {
  return syscall(__NR_riscv_flush_icache, reinterpret_cast<intptr_t>(start),
                 reinterpret_cast<intptr_t>(end), static_cast<intptr_t>(flags));
}
