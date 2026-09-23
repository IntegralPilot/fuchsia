// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstddef>
#include <cstdint>

#include <asm/unistd.h>

// This test examines the initial extended processor state when entering a new process.
// This means it must start at _start and not depend on any of the usual initialization logic
// performed by libc that may clobber this state before we have a chance to examine it. Thus
// it has some assembly helpers to issue syscalls standalone.

// Generic syscall with 4 arguments.
intptr_t syscall4(intptr_t syscall_number, intptr_t arg1, intptr_t arg2, intptr_t arg3,
                  intptr_t arg4) {
  intptr_t ret;
#if defined(__x86_64__)
  register intptr_t r10 asm("r10") = arg4;
  __asm__ volatile("syscall;"
                   : "=a"(ret)
                   : "a"(syscall_number), "D"(arg1), "S"(arg2), "d"(arg3), "r"(r10)
                   : "rcx", "r11", "memory");
#elif defined(__arm__)
  register intptr_t r0 asm("r0") = arg1;
  register intptr_t r1 asm("r1") = arg2;
  register intptr_t r2 asm("r2") = arg3;
  register intptr_t r3 asm("r3") = arg4;
  register intptr_t number asm("r7") = syscall_number;

  __asm__ volatile("svc #0"
                   : "=r"(ret)
                   : "0"(r0), "r"(r1), "r"(r2), "r"(r3), "r"(number)
                   : "memory");
#elif defined(__aarch64__)
  register intptr_t x0 asm("x0") = arg1;
  register intptr_t x1 asm("x1") = arg2;
  register intptr_t x2 asm("x2") = arg3;
  register intptr_t x3 asm("x3") = arg4;
  register intptr_t number asm("x8") = syscall_number;

  __asm__ volatile("svc #0"
                   : "=r"(ret)
                   : "0"(x0), "r"(x1), "r"(x2), "r"(x3), "r"(number)
                   : "memory");
#elif defined(__riscv)
  register intptr_t a0 asm("a0") = arg1;
  register intptr_t a1 asm("a1") = arg2;
  register intptr_t a2 asm("a2") = arg3;
  register intptr_t a3 asm("a3") = arg4;
  register intptr_t number asm("a7") = syscall_number;

  __asm__ volatile("ecall"
                   : "=r"(ret)
                   : "r"(a0), "r"(a1), "r"(a2), "r"(a3), "r"(number)
                   : "memory");
#else
#error Unsupported architecture
#endif
  return ret;
}

#define SYSCCALL4(number, arg1, arg2, arg3, arg4, ...) \
  syscall4(number, (intptr_t)(arg1), (intptr_t)(arg2), (intptr_t)(arg3), (intptr_t)(arg4))

#define SYSCALL(number, ...) SYSCCALL4(number, __VA_ARGS__, 0, 0, 0, 0)

void exit_group(int exit_code) { SYSCALL(__NR_exit_group, exit_code); }

void exit_success() { exit_group(0); }

size_t strlen(const char* str) {
  const char* end = str;
  while (*end)
    ++end;
  return end - str;
}

void write_stderr(const char* str, size_t len) { SYSCALL(__NR_write, 2, str, len); }

void fail(const char* msg, int exit_code) {
  write_stderr(msg, strlen(msg));
  exit_group(exit_code);
}

extern "C" void _start() {
#if defined(__x86_64__)
  constexpr size_t kXmmRegisterWidth = 128;
  std::byte buffer[kXmmRegisterWidth * 16];

#define MOVUPS_XMM_TO_ADDR(reg, addr) asm("movups %%xmm" #reg ", %0" : : "m"(addr) : "memory");
  MOVUPS_XMM_TO_ADDR(0, buffer[0 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(1, buffer[1 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(2, buffer[2 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(3, buffer[3 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(4, buffer[4 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(5, buffer[5 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(6, buffer[6 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(7, buffer[7 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(8, buffer[8 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(9, buffer[9 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(10, buffer[10 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(11, buffer[11 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(12, buffer[12 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(13, buffer[13 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(14, buffer[14 * kXmmRegisterWidth]);
  MOVUPS_XMM_TO_ADDR(15, buffer[15 * kXmmRegisterWidth]);
#undef MOVUPS_XMM_TO_ADDR

  for (size_t i = 0; i < kXmmRegisterWidth * 16; ++i) {
    if (buffer[i] != std::byte{0}) {
      int register_number = static_cast<int>(i) / kXmmRegisterWidth;
      char message[] = "XMM00\n";
      message[4] = '0' + register_number % 10;
      message[3] = '0' + (register_number / 10) % 10;
      fail(message, static_cast<int>(i / kXmmRegisterWidth) + 1);
    }
  }
#elif defined(__arm__)
  constexpr size_t kDRegisterWidth = 8;
  std::byte d_register_buffer[32 * kDRegisterWidth];
  uint32_t fpscr = 0;
  __asm__ volatile(
      "vstr       d0, [%1, #(0 * 8)]\n"
      "vstr       d1, [%1, #(1 * 8)]\n"
      "vstr       d2, [%1, #(2 * 8)]\n"
      "vstr       d3, [%1, #(3 * 8)]\n"
      "vstr       d4, [%1, #(4 * 8)]\n"
      "vstr       d5, [%1, #(5 * 8)]\n"
      "vstr       d6, [%1, #(6 * 8)]\n"
      "vstr       d7, [%1, #(7 * 8)]\n"
      "vstr       d8, [%1, #(8 * 8)]\n"
      "vstr       d9, [%1, #(9 * 8)]\n"
      "vstr      d10, [%1, #(10 * 8)]\n"
      "vstr      d11, [%1, #(11 * 8)]\n"
      "vstr      d12, [%1, #(12 * 8)]\n"
      "vstr      d13, [%1, #(13 * 8)]\n"
      "vstr      d14, [%1, #(14 * 8)]\n"
      "vstr      d15, [%1, #(15 * 8)]\n"
      "vstr      d16, [%1, #(16 * 8)]\n"
      "vstr      d17, [%1, #(17 * 8)]\n"
      "vstr      d18, [%1, #(18 * 8)]\n"
      "vstr      d19, [%1, #(19 * 8)]\n"
      "vstr      d20, [%1, #(20 * 8)]\n"
      "vstr      d21, [%1, #(21 * 8)]\n"
      "vstr      d22, [%1, #(22 * 8)]\n"
      "vstr      d23, [%1, #(23 * 8)]\n"
      "vstr      d24, [%1, #(24 * 8)]\n"
      "vstr      d25, [%1, #(25 * 8)]\n"
      "vstr      d26, [%1, #(26 * 8)]\n"
      "vstr      d27, [%1, #(27 * 8)]\n"
      "vstr      d28, [%1, #(28 * 8)]\n"
      "vstr      d29, [%1, #(29 * 8)]\n"
      "vstr      d30, [%1, #(30 * 8)]\n"
      "vstr      d31, [%1, #(31 * 8)]\n"
      "vmrs      %0, fpscr\n"
      : "=r"(fpscr)
      : "r"(d_register_buffer)
      : "memory");
  for (size_t i = 0; i < 32 * kDRegisterWidth; ++i) {
    if (d_register_buffer[i] != std::byte{0}) {
      int register_number = static_cast<int>(i / kDRegisterWidth);
      char message[] = "D00\n";
      message[2] = '0' + register_number % 10;
      message[1] = '0' + (register_number / 10) % 10;
      fail(message, register_number + 1);
    }
  }
  if (fpscr != 0) {
    fail("fpscr\n", 16);
  }
#elif defined(__aarch64__)
  constexpr size_t kQRegisterWidth = 128;
  std::byte q_register_buffer[32 * kQRegisterWidth];
  uint64_t fpcr = 0;
  uint64_t fpsr = 0;
  __asm__ volatile(
      "stp      q0,  q1, [%2, #(0 * 32)]\n"
      "stp      q2,  q3, [%2, #(1 * 32)]\n"
      "stp      q4,  q5, [%2, #(2 * 32)]\n"
      "stp      q6,  q7, [%2, #(3 * 32)]\n"
      "stp      q8,  q9, [%2, #(4 * 32)]\n"
      "stp     q10, q11, [%2, #(5 * 32)]\n"
      "stp     q12, q13, [%2, #(6 * 32)]\n"
      "stp     q14, q15, [%2, #(7 * 32)]\n"
      "stp     q16, q17, [%2, #(8 * 32)]\n"
      "stp     q18, q19, [%2, #(9 * 32)]\n"
      "stp     q20, q21, [%2, #(10 * 32)]\n"
      "stp     q22, q23, [%2, #(11 * 32)]\n"
      "stp     q24, q25, [%2, #(12 * 32)]\n"
      "stp     q26, q27, [%2, #(13 * 32)]\n"
      "stp     q28, q29, [%2, #(14 * 32)]\n"
      "stp     q30, q31, [%2, #(15 * 32)]\n"
      "mrs     %0, fpcr\n"
      "mrs     %1, fpsr\n"
      : "=r"(fpcr), "=r"(fpsr)
      : "r"(q_register_buffer)
      : "memory");
  for (size_t i = 0; i < 32 * kQRegisterWidth; ++i) {
    if (q_register_buffer[i] != std::byte{0}) {
      int register_number = static_cast<int>(i / kQRegisterWidth);
      char message[] = "Q00\n";
      message[2] = '0' + register_number % 10;
      message[1] = '0' + (register_number / 10) % 10;
      fail(message, register_number + 1);
    }
  }
  if (fpcr != 0) {
    fail("fpcr\n", 33);
  }
  if (fpsr != 0) {
    fail("fpsr\n", 34);
  }
#elif defined(__riscv)
  uint64_t fp_registers[32];
  uint32_t fcsr;
  std::byte v_registers[32 * 16];  // Assuming VLEN=128

  __asm__ volatile(
      "fld  f0,  0 * 8(%[regs])\n"
      "fld  f1,  1 * 8(%[regs])\n"
      "fld  f2,  2 * 8(%[regs])\n"
      "fld  f3,  3 * 8(%[regs])\n"
      "fld  f4,  4 * 8(%[regs])\n"
      "fld  f5,  5 * 8(%[regs])\n"
      "fld  f6,  6 * 8(%[regs])\n"
      "fld  f7,  7 * 8(%[regs])\n"
      "fld  f8,  8 * 8(%[regs])\n"
      "fld  f9,  9 * 8(%[regs])\n"
      "fld f10, 10 * 8(%[regs])\n"
      "fld f11, 11 * 8(%[regs])\n"
      "fld f12, 12 * 8(%[regs])\n"
      "fld f13, 13 * 8(%[regs])\n"
      "fld f14, 14 * 8(%[regs])\n"
      "fld f15, 15 * 8(%[regs])\n"
      "fld f16, 16 * 8(%[regs])\n"
      "fld f17, 17 * 8(%[regs])\n"
      "fld f18, 18 * 8(%[regs])\n"
      "fld f19, 19 * 8(%[regs])\n"
      "fld f20, 20 * 8(%[regs])\n"
      "fld f21, 21 * 8(%[regs])\n"
      "fld f22, 22 * 8(%[regs])\n"
      "fld f23, 23 * 8(%[regs])\n"
      "fld f24, 24 * 8(%[regs])\n"
      "fld f25, 25 * 8(%[regs])\n"
      "fld f26, 26 * 8(%[regs])\n"
      "fld f27, 27 * 8(%[regs])\n"
      "fld f28, 28 * 8(%[regs])\n"
      "fld f29, 29 * 8(%[regs])\n"
      "fld f30, 30 * 8(%[regs])\n"
      "fld f31, 31 * 8(%[regs])\n"
      "frcsr %[fcsr]\n"
      // Save 32 vector registers. Since we can only save 8 registers at a time with `vs8r.v`,
      // we do it in 4 chunks of 8, incrementing the pointer by the size of 8 vector registers
      // (`vlenb * 8` bytes) each time.
      "csrr t0, vlenb\n"
      "slli t0, t0, 3\n"
      "vs8r.v v0, (%[vregs])\n"
      "add %[vregs], %[vregs], t0\n"
      "vs8r.v v8, (%[vregs])\n"
      "add %[vregs], %[vregs], t0\n"
      "vs8r.v v16, (%[vregs])\n"
      "add %[vregs], %[vregs], t0\n"
      "vs8r.v v24, (%[vregs])\n"
      : [fcsr] "=r"(fcsr)
      : [regs] "r"(fp_registers), [vregs] "r"(v_registers)
      : "t0", "memory");
  for (size_t i = 0; i < 32; ++i) {
    if (fp_registers[i] != 0) {
      char message[] = "F00\n";
      char register_number = static_cast<char>(i);
      message[2] = '0' + register_number % 10;
      message[1] = '0' + register_number / 10;
      fail(message, register_number + 1);
    }
  }
  if (fcsr != 0) {
    fail("fcsr\n", 33);
  }
  // Verify that all bytes of the 32 vector registers are 0.
  for (size_t i = 0; i < 32 * 16; ++i) {
    if (v_registers[i] != std::byte{0}) {
      int register_number = static_cast<int>(i / 16);
      char message[] = "V00\n";
      message[2] = '0' + register_number % 10;
      message[1] = '0' + (register_number / 10) % 10;
      fail(message, register_number + 35);
    }
  }
#else
#error "unimplemented"
#endif
  exit_success();
}
