// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

#if defined(__riscv)
// Only VLEN=128 is supported currently.
const size_t RISCV_VLEN = 128;

// The following constants and structs duplicate the definitions that are
// available only with newer UAPI headers.
// TODO(b/325538967): Remove once we the headers used to compile this test are updated.
#define RISCV_V_MAGIC 0x53465457U
#define END_MAGIC 0x0U
#define END_HDR_SIZE 0x0U

struct riscv_ctx_hdr {
  uint32_t magic;
  uint32_t size;
};

struct riscv_v_ext_state {
  uint64_t vstart;
  uint64_t vl;
  uint64_t vtype;
  uint64_t vcsr;
  uint64_t vlenb;
  void* datap;
};
#endif  // defined(__riscv)

#if defined(__x86_64__)
struct RegistersValue {
  __uint128_t xmm[16];
  bool operator==(const RegistersValue& other) const {
    return memcmp(this, &other, sizeof(RegistersValue)) == 0;
  }
};
#elif defined(__arm__)
struct RegistersValue {
  uint64_t d[32];
  bool operator==(const RegistersValue& other) const {
    return memcmp(this, &other, sizeof(RegistersValue)) == 0;
  }
};
#elif defined(__aarch64__)
struct RegistersValue {
  __uint128_t q[32];
  bool operator==(const RegistersValue& other) const {
    return memcmp(this, &other, sizeof(RegistersValue)) == 0;
  }
};
#elif defined(__riscv)
struct RegistersValue {
  uint64_t f[32];
  __uint128_t v[32];  // Assuming VLEN=128
  bool operator==(const RegistersValue& other) const {
    return memcmp(this, &other, sizeof(RegistersValue)) == 0;
  }
};
#else
#error Add support for this architecture
#endif

// Stores a 128-bit value in SIMD registers used for the test.
void SetTestRegisters(const RegistersValue* value) {
#if defined(__x86_64__)
  asm volatile(
      "movups   0(%0), %%xmm0\n"
      "movups  16(%0), %%xmm1\n"
      "movups  32(%0), %%xmm2\n"
      "movups  48(%0), %%xmm3\n"
      "movups  64(%0), %%xmm4\n"
      "movups  80(%0), %%xmm5\n"
      "movups  96(%0), %%xmm6\n"
      "movups 112(%0), %%xmm7\n"
      "movups 128(%0), %%xmm8\n"
      "movups 144(%0), %%xmm9\n"
      "movups 160(%0), %%xmm10\n"
      "movups 176(%0), %%xmm11\n"
      "movups 192(%0), %%xmm12\n"
      "movups 208(%0), %%xmm13\n"
      "movups 224(%0), %%xmm14\n"
      "movups 240(%0), %%xmm15\n"
      :
      : "r"(value));
#elif defined(__arm__)
  asm volatile(
      "vldr d0, [%0, #0]\n"
      "vldr d1, [%0, #8]\n"
      "vldr d2, [%0, #16]\n"
      "vldr d3, [%0, #24]\n"
      "vldr d4, [%0, #32]\n"
      "vldr d5, [%0, #40]\n"
      "vldr d6, [%0, #48]\n"
      "vldr d7, [%0, #56]\n"
      "vldr d8, [%0, #64]\n"
      "vldr d9, [%0, #72]\n"
      "vldr d10, [%0, #80]\n"
      "vldr d11, [%0, #88]\n"
      "vldr d12, [%0, #96]\n"
      "vldr d13, [%0, #104]\n"
      "vldr d14, [%0, #112]\n"
      "vldr d15, [%0, #120]\n"
      "vldr d16, [%0, #128]\n"
      "vldr d17, [%0, #136]\n"
      "vldr d18, [%0, #144]\n"
      "vldr d19, [%0, #152]\n"
      "vldr d20, [%0, #160]\n"
      "vldr d21, [%0, #168]\n"
      "vldr d22, [%0, #176]\n"
      "vldr d23, [%0, #184]\n"
      "vldr d24, [%0, #192]\n"
      "vldr d25, [%0, #200]\n"
      "vldr d26, [%0, #208]\n"
      "vldr d27, [%0, #216]\n"
      "vldr d28, [%0, #224]\n"
      "vldr d29, [%0, #232]\n"
      "vldr d30, [%0, #240]\n"
      "vldr d31, [%0, #248]\n"
      :
      : "r"(value));
#elif defined(__aarch64__)
  asm volatile(
      "ldr q0, [%0, #0]\n"
      "ldr q1, [%0, #16]\n"
      "ldr q2, [%0, #32]\n"
      "ldr q3, [%0, #48]\n"
      "ldr q4, [%0, #64]\n"
      "ldr q5, [%0, #80]\n"
      "ldr q6, [%0, #96]\n"
      "ldr q7, [%0, #112]\n"
      "ldr q8, [%0, #128]\n"
      "ldr q9, [%0, #144]\n"
      "ldr q10, [%0, #160]\n"
      "ldr q11, [%0, #176]\n"
      "ldr q12, [%0, #192]\n"
      "ldr q13, [%0, #208]\n"
      "ldr q14, [%0, #224]\n"
      "ldr q15, [%0, #240]\n"
      "ldr q16, [%0, #256]\n"
      "ldr q17, [%0, #272]\n"
      "ldr q18, [%0, #288]\n"
      "ldr q19, [%0, #304]\n"
      "ldr q20, [%0, #320]\n"
      "ldr q21, [%0, #336]\n"
      "ldr q22, [%0, #352]\n"
      "ldr q23, [%0, #368]\n"
      "ldr q24, [%0, #384]\n"
      "ldr q25, [%0, #400]\n"
      "ldr q26, [%0, #416]\n"
      "ldr q27, [%0, #432]\n"
      "ldr q28, [%0, #448]\n"
      "ldr q29, [%0, #464]\n"
      "ldr q30, [%0, #480]\n"
      "ldr q31, [%0, #496]\n"
      :
      : "r"(value));
#elif defined(__riscv)
  asm volatile(
      "fld f0, 0(%0)\n"
      "fld f1, 8(%0)\n"
      "fld f2, 16(%0)\n"
      "fld f3, 24(%0)\n"
      "fld f4, 32(%0)\n"
      "fld f5, 40(%0)\n"
      "fld f6, 48(%0)\n"
      "fld f7, 56(%0)\n"
      "fld f8, 64(%0)\n"
      "fld f9, 72(%0)\n"
      "fld f10, 80(%0)\n"
      "fld f11, 88(%0)\n"
      "fld f12, 96(%0)\n"
      "fld f13, 104(%0)\n"
      "fld f14, 112(%0)\n"
      "fld f15, 120(%0)\n"
      "fld f16, 128(%0)\n"
      "fld f17, 136(%0)\n"
      "fld f18, 144(%0)\n"
      "fld f19, 152(%0)\n"
      "fld f20, 160(%0)\n"
      "fld f21, 168(%0)\n"
      "fld f22, 176(%0)\n"
      "fld f23, 184(%0)\n"
      "fld f24, 192(%0)\n"
      "fld f25, 200(%0)\n"
      "fld f26, 208(%0)\n"
      "fld f27, 216(%0)\n"
      "fld f28, 224(%0)\n"
      "fld f29, 232(%0)\n"
      "fld f30, 240(%0)\n"
      "fld f31, 248(%0)\n"
      "addi %0, %0, 256\n"
      // Load 32 vector registers. Since we can only load 8 registers at a time with `vl8r.v`,
      // we do it in 4 chunks of 8, incrementing the pointer by the size of 8 vector registers
      // (`vlenb * 8` bytes) each time.
      "csrr t0, vlenb\n"
      "slli t0, t0, 3\n"
      "vl8r.v v0, (%0)\n"
      "add %0, %0, t0\n"
      "vl8r.v v8, (%0)\n"
      "add %0, %0, t0\n"
      "vl8r.v v16, (%0)\n"
      "add %0, %0, t0\n"
      "vl8r.v v24, (%0)\n"
      :
      : "r"(value)
      : "t0");
#else
#error Add support for this architecture
#endif
}

// Reads a 128-bit value from the SIMD registers that were set in SetTestRegisters().
RegistersValue GetTestRegisters() {
  RegistersValue value;

#if defined(__x86_64__)
  asm volatile(
      "movups %%xmm0,   0(%0)\n"
      "movups %%xmm1,  16(%0)\n"
      "movups %%xmm2,  32(%0)\n"
      "movups %%xmm3,  48(%0)\n"
      "movups %%xmm4,  64(%0)\n"
      "movups %%xmm5,  80(%0)\n"
      "movups %%xmm6,  96(%0)\n"
      "movups %%xmm7, 112(%0)\n"
      "movups %%xmm8, 128(%0)\n"
      "movups %%xmm9, 144(%0)\n"
      "movups %%xmm10, 160(%0)\n"
      "movups %%xmm11, 176(%0)\n"
      "movups %%xmm12, 192(%0)\n"
      "movups %%xmm13, 208(%0)\n"
      "movups %%xmm14, 224(%0)\n"
      "movups %%xmm15, 240(%0)\n"
      :
      : "r"(&value));
#elif defined(__arm__)
  asm volatile(
      "vstr d0, [%0, #0]\n"
      "vstr d1, [%0, #8]\n"
      "vstr d2, [%0, #16]\n"
      "vstr d3, [%0, #24]\n"
      "vstr d4, [%0, #32]\n"
      "vstr d5, [%0, #40]\n"
      "vstr d6, [%0, #48]\n"
      "vstr d7, [%0, #56]\n"
      "vstr d8, [%0, #64]\n"
      "vstr d9, [%0, #72]\n"
      "vstr d10, [%0, #80]\n"
      "vstr d11, [%0, #88]\n"
      "vstr d12, [%0, #96]\n"
      "vstr d13, [%0, #104]\n"
      "vstr d14, [%0, #112]\n"
      "vstr d15, [%0, #120]\n"
      "vstr d16, [%0, #128]\n"
      "vstr d17, [%0, #136]\n"
      "vstr d18, [%0, #144]\n"
      "vstr d19, [%0, #152]\n"
      "vstr d20, [%0, #160]\n"
      "vstr d21, [%0, #168]\n"
      "vstr d22, [%0, #176]\n"
      "vstr d23, [%0, #184]\n"
      "vstr d24, [%0, #192]\n"
      "vstr d25, [%0, #200]\n"
      "vstr d26, [%0, #208]\n"
      "vstr d27, [%0, #216]\n"
      "vstr d28, [%0, #224]\n"
      "vstr d29, [%0, #232]\n"
      "vstr d30, [%0, #240]\n"
      "vstr d31, [%0, #248]\n"
      :
      : "r"(&value));
#elif defined(__aarch64__)
  asm volatile(
      "str q0, [%0, #0]\n"
      "str q1, [%0, #16]\n"
      "str q2, [%0, #32]\n"
      "str q3, [%0, #48]\n"
      "str q4, [%0, #64]\n"
      "str q5, [%0, #80]\n"
      "str q6, [%0, #96]\n"
      "str q7, [%0, #112]\n"
      "str q8, [%0, #128]\n"
      "str q9, [%0, #144]\n"
      "str q10, [%0, #160]\n"
      "str q11, [%0, #176]\n"
      "str q12, [%0, #192]\n"
      "str q13, [%0, #208]\n"
      "str q14, [%0, #224]\n"
      "str q15, [%0, #240]\n"
      "str q16, [%0, #256]\n"
      "str q17, [%0, #272]\n"
      "str q18, [%0, #288]\n"
      "str q19, [%0, #304]\n"
      "str q20, [%0, #320]\n"
      "str q21, [%0, #336]\n"
      "str q22, [%0, #352]\n"
      "str q23, [%0, #368]\n"
      "str q24, [%0, #384]\n"
      "str q25, [%0, #400]\n"
      "str q26, [%0, #416]\n"
      "str q27, [%0, #432]\n"
      "str q28, [%0, #448]\n"
      "str q29, [%0, #464]\n"
      "str q30, [%0, #480]\n"
      "str q31, [%0, #496]\n"
      :
      : "r"(&value));
#elif defined(__riscv)
  asm volatile(
      "fsd f0, 0(%0)\n"
      "fsd f1, 8(%0)\n"
      "fsd f2, 16(%0)\n"
      "fsd f3, 24(%0)\n"
      "fsd f4, 32(%0)\n"
      "fsd f5, 40(%0)\n"
      "fsd f6, 48(%0)\n"
      "fsd f7, 56(%0)\n"
      "fsd f8, 64(%0)\n"
      "fsd f9, 72(%0)\n"
      "fsd f10, 80(%0)\n"
      "fsd f11, 88(%0)\n"
      "fsd f12, 96(%0)\n"
      "fsd f13, 104(%0)\n"
      "fsd f14, 112(%0)\n"
      "fsd f15, 120(%0)\n"
      "fsd f16, 128(%0)\n"
      "fsd f17, 136(%0)\n"
      "fsd f18, 144(%0)\n"
      "fsd f19, 152(%0)\n"
      "fsd f20, 160(%0)\n"
      "fsd f21, 168(%0)\n"
      "fsd f22, 176(%0)\n"
      "fsd f23, 184(%0)\n"
      "fsd f24, 192(%0)\n"
      "fsd f25, 200(%0)\n"
      "fsd f26, 208(%0)\n"
      "fsd f27, 216(%0)\n"
      "fsd f28, 224(%0)\n"
      "fsd f29, 232(%0)\n"
      "fsd f30, 240(%0)\n"
      "fsd f31, 248(%0)\n"
      "addi %0, %0, 256\n"
      // Save 32 vector registers. Since we can only save 8 registers at a time with `vs8r.v`,
      // we do it in 4 chunks of 8, incrementing the pointer by the size of 8 vector registers
      // (`vlenb * 8` bytes) each time.
      "csrr t0, vlenb\n"
      "slli t0, t0, 3\n"
      "vs8r.v v0, (%0)\n"
      "add %0, %0, t0\n"
      "vs8r.v v8, (%0)\n"
      "add %0, %0, t0\n"
      "vs8r.v v16, (%0)\n"
      "add %0, %0, t0\n"
      "vs8r.v v24, (%0)\n"
      :
      : "r"(&value)
      : "t0");
#else
#error Add support for this architecture
#endif

  return value;
}

// FP/SIMD registers should be initialized to 0 for new processes.
TEST(ExtendedPstate, InitialState) {
  std::string child_path = test_helper::GetTestResourcePath("extended_pstate_initial_state_child");
  ASSERT_TRUE(files::IsFile(child_path)) << child_path;
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&child_path] {
    // Set some registers. execve() should reset them to 0.
    RegistersValue kTestData;
#if defined(__x86_64__)
    for (int i = 0; i < 16; ++i)
      kTestData.xmm[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x090a0b0c0d0e0f10ULL + i);
#elif defined(__arm__)
    for (int i = 0; i < 32; ++i)
      kTestData.d[i] = 0x0102030405060708ULL + i;
#elif defined(__aarch64__)
    for (int i = 0; i < 32; ++i)
      kTestData.q[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x090a0b0c0d0e0f10ULL + i);
#elif defined(__riscv)
    for (int i = 0; i < 32; ++i) {
      kTestData.f[i] = 0x0102030405060708ULL + i;
      kTestData.v[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
    }
#endif
    SetTestRegisters(&kTestData);

    char* argv[] = {nullptr};
    char* envp[] = {nullptr};
    ASSERT_EQ(execve(child_path.c_str(), argv, envp), 0)
        << "execve error: " << errno << " (" << strerror(errno) << ")";
  });
}

// Verify that FP/SIMD registers are preserved by syscalls.
TEST(ExtendedPstate, Syscall) {
  RegistersValue kTestRegisters;
#if defined(__x86_64__)
  for (int i = 0; i < 16; ++i)
    kTestRegisters.xmm[i] =
        (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
#elif defined(__arm__)
  for (int i = 0; i < 32; ++i)
    kTestRegisters.d[i] = 0x0102030405060708ULL + i;
#elif defined(__aarch64__)
  for (int i = 0; i < 32; ++i)
    kTestRegisters.q[i] =
        (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x090a0b0c0d0e0f10ULL + i);
#elif defined(__riscv)
  for (int i = 0; i < 32; ++i) {
    kTestRegisters.f[i] = 0x0102030405060708ULL + i;
    kTestRegisters.v[i] =
        (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
  }
#endif

  SetTestRegisters(&kTestRegisters);

  // Make several syscalls. Kernel uses floating point to generate `/proc/uptime` content, which
  // may affect the registers being tested.
  int fd = open("/proc/uptime", O_RDONLY);
  EXPECT_GT(fd, 0);
  char c;
  EXPECT_EQ(read(fd, &c, 1), 1);
  EXPECT_EQ(close(fd), 0);

  EXPECT_EQ(GetTestRegisters(), kTestRegisters);
}

struct SignalHandlerData {
  void* sigsegv_target;
  bool received_sigsegv = false;
  RegistersValue sigsegv_regs;

  RegistersValue sigusr1_regs;
};

#if defined(__arm__)
// Layout of the ucontext struct that allows access to the pstate
struct __attribute__((__packed__)) ucontext_with_pstate {
  char prefix[124];
  RegistersValue pstate;
};
#endif

RegistersValue GetTestRegistersFromUcontext(ucontext_t* ucontext) {
  RegistersValue result;

#if defined(__x86_64__)
  auto fpregs = ucontext->uc_mcontext.fpregs;
  memcpy(&result, reinterpret_cast<void*>(fpregs->_xmm), sizeof(result));
  auto fpstate_ptr = reinterpret_cast<char*>(fpregs);

  // Bytes 464..512 in the XSAVE area are not used by XSAVE. Linux uses these bytes to store
  // `struct _fpx_sw_bytes`, which declares the set of extensions that may follow immediately
  // after `fpstate`. The region is marked with two "magic" values. Check that they are set
  // correctly.
  auto sw_bytes = reinterpret_cast<_fpx_sw_bytes*>(fpstate_ptr + 464);
  EXPECT_EQ(sw_bytes->magic1, FP_XSTATE_MAGIC1);
  uint32_t* magic2_ptr =
      reinterpret_cast<uint32_t*>(fpstate_ptr + sw_bytes->extended_size - FP_XSTATE_MAGIC2_SIZE);
  EXPECT_EQ(*magic2_ptr, FP_XSTATE_MAGIC2);
#elif defined(__arm__)
  ucontext_with_pstate* fp_context = reinterpret_cast<ucontext_with_pstate*>(ucontext);
  memcpy(&result, &fp_context->pstate, sizeof(result));
#elif defined(__aarch64__)
  fpsimd_context* fp_context = reinterpret_cast<fpsimd_context*>(ucontext->uc_mcontext.__reserved);
  EXPECT_EQ(fp_context->head.magic, static_cast<uint32_t>(FPSIMD_MAGIC));
  EXPECT_EQ(fp_context->head.size, sizeof(fpsimd_context));
  memcpy(&result, fp_context->vregs, sizeof(result));
#elif defined(__riscv)
  // Copy 32 F registers
  memcpy(&result.f, reinterpret_cast<void*>(ucontext->uc_mcontext.__fpregs.__d.__f),
         sizeof(result.f));

  // The header for the first RISC-V context extension is at the end of `uc_mcontext`.
  riscv_ctx_hdr* hdr = reinterpret_cast<riscv_ctx_hdr*>(&(ucontext->uc_mcontext) + 1) - 1;

  EXPECT_EQ(hdr->magic, RISCV_V_MAGIC);
  // Assuming VLEN=128 we meed 512 bytes to store 32 V registers.
  EXPECT_EQ(hdr->size, sizeof(riscv_ctx_hdr) + sizeof(riscv_v_ext_state) + 512);

  riscv_v_ext_state* v_state = reinterpret_cast<riscv_v_ext_state*>(hdr + 1);
  EXPECT_EQ(v_state->vlenb, RISCV_VLEN / 8);
  EXPECT_EQ(hdr->size, sizeof(riscv_ctx_hdr) + sizeof(riscv_v_ext_state) + RISCV_VLEN / 8 * 32);
  EXPECT_EQ(v_state->datap, v_state + 1);

  // Copy 32 V registers
  memcpy(&result.v, reinterpret_cast<void*>(v_state->datap), sizeof(result.v));

  riscv_ctx_hdr* end_hdr =
      reinterpret_cast<riscv_ctx_hdr*>(reinterpret_cast<uint8_t*>(hdr) + hdr->size);
  EXPECT_EQ(end_hdr->magic, END_MAGIC);
  EXPECT_EQ(end_hdr->size, END_HDR_SIZE);
#else
#error Add support for this architecture
#endif

  return result;
}

SignalHandlerData signal_data;

// FP/SIMD registers are expected to be restored when returning form signal handlers
TEST(ExtendedPstate, Signals) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    void* target = mmap(nullptr, page_size, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(target, MAP_FAILED);
    signal_data.sigsegv_target = target;

    // Register SIGUSR1 handler.
    struct sigaction sigusr1_action = {};
    sigusr1_action.sa_sigaction = [](int sig, siginfo_t* info, void* ucontext) {
      if (sig != SIGUSR1) {
        _exit(1);
      }
      signal_data.sigusr1_regs =
          GetTestRegistersFromUcontext(reinterpret_cast<ucontext_t*>(ucontext));

      // Reset registers content.
      RegistersValue zero = {};
      SetTestRegisters(&zero);
    };
    sigusr1_action.sa_flags = SA_SIGINFO;
    SAFE_SYSCALL(sigaction(SIGUSR1, &sigusr1_action, nullptr));

    // Register SIGSEGV handler.
    struct sigaction sigsegv_action = {};
    signal_data.received_sigsegv = false;
    sigsegv_action.sa_sigaction = [](int sig, siginfo_t* info, void* ucontext) {
      if (sig != SIGSEGV || info->si_addr != signal_data.sigsegv_target) {
        _exit(1);
      }
      signal_data.received_sigsegv = true;
      signal_data.sigsegv_regs =
          GetTestRegistersFromUcontext(reinterpret_cast<ucontext_t*>(ucontext));

      // Set registers to a value different from what it was outside of the signal handler.
      RegistersValue kNestedRegs;
#if defined(__x86_64__)
      for (int i = 0; i < 16; ++i)
        kNestedRegs.xmm[i] = (static_cast<__uint128_t>(0x191a1b1c1d1e1f20ULL + i) << 64) |
                             (0x1112131415161718ULL + i);
#elif defined(__arm__)
      for (int i = 0; i < 32; ++i)
        kNestedRegs.d[i] = 0x191a1b1c1d1e1f20ULL + i;
#elif defined(__aarch64__)
      for (int i = 0; i < 32; ++i)
        kNestedRegs.q[i] = (static_cast<__uint128_t>(0x191a1b1c1d1e1f20ULL + i) << 64) |
                           (0x1112131415161718ULL + i);
#elif defined(__riscv)
      for (int i = 0; i < 32; ++i) {
        kNestedRegs.f[i] = 0x191a1b1c1d1e1f20ULL + i;
        kNestedRegs.v[i] = (static_cast<__uint128_t>(0x191a1b1c1d1e1f20ULL + i) << 64) |
                           (0x1112131415161718ULL + i);
      }
#endif
      SetTestRegisters(&kNestedRegs);

      // Raise another signal.
      raise(SIGUSR1);

      // Nested signal handler should preserve all registers.
      EXPECT_EQ(GetTestRegisters(), kNestedRegs);

      // Nested signal handler should receive values at the time it was invoked.
      EXPECT_EQ(signal_data.sigusr1_regs, kNestedRegs);

      // TODO: mprotect is not listed in signal-safety(7), should issue raw syscall
      mprotect(info->si_addr, 4096, PROT_READ | PROT_WRITE);
    };
    sigsegv_action.sa_flags = SA_SIGINFO;
    SAFE_SYSCALL(sigaction(SIGSEGV, &sigsegv_action, nullptr));

    RegistersValue kTestRegsValue;
#if defined(__x86_64__)
    for (int i = 0; i < 16; ++i)
      kTestRegsValue.xmm[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x090a0b0c0d0e0f10ULL + i);
#elif defined(__arm__)
    for (int i = 0; i < 32; ++i)
      kTestRegsValue.d[i] = 0x0102030405060708ULL + i;
#elif defined(__aarch64__)
    for (int i = 0; i < 32; ++i)
      kTestRegsValue.q[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
#elif defined(__riscv)
    for (int i = 0; i < 32; ++i) {
      kTestRegsValue.f[i] = 0x0102030405060708ULL + i;
      kTestRegsValue.v[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
    }
#endif
    SetTestRegisters(&kTestRegsValue);

    // Issue a store that will generate fault which will be fixed in SIGSEGV handler.
    asm volatile("" ::: "memory");
    *static_cast<char*>(target) = 1;
    asm volatile("" ::: "memory");
    ASSERT_TRUE(signal_data.received_sigsegv);

    // Check that the SIMD registers were preserved.
    EXPECT_EQ(GetTestRegisters(), kTestRegsValue);

    // Validate the registers value passed to the SIGSEGV handler in ucontext.
    EXPECT_EQ(signal_data.sigsegv_regs, kTestRegsValue);
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

// Verify that FP/SIMD registers are preserved across recoverable page faults (stack growth).
TEST(ExtendedPstate, StackGrowth) {
  // Run in a child to ensure only the one test thread is mutating the address space.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Find a 4-page "opening" in the address space that a 2-page mapping can grow into by making it
    // ourselves. Since we're the only thread, we know that mapping and then immediately unmapping
    // will leave the space reserved.
    void* base_addr = mmap(nullptr, 4 * page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_NE(base_addr, MAP_FAILED);
    SAFE_SYSCALL(munmap(base_addr, 4 * page_size));

    // Start our down-growing mapping in the middle of the reserved space.
    void* grow_low =
        mmap(reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(base_addr) + (2 * page_size)),
             2 * page_size, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_GROWSDOWN, -1, 0);
    ASSERT_NE(grow_low, MAP_FAILED);

    RegistersValue kTestRegisters;
#if defined(__x86_64__)
    for (int i = 0; i < 16; ++i) {
      kTestRegisters.xmm[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x090a0b0c0d0e0f10ULL + i);
    }
#elif defined(__arm__)
    for (int i = 0; i < 32; ++i) {
      kTestRegisters.d[i] = 0x0102030405060708ULL + i;
    }
#elif defined(__aarch64__)
    for (int i = 0; i < 32; ++i) {
      kTestRegisters.q[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
    }
#elif defined(__riscv)
    for (int i = 0; i < 32; ++i) {
      kTestRegisters.f[i] = 0x0102030405060708ULL + i;
      kTestRegisters.v[i] =
          (static_cast<__uint128_t>(0x0102030405060708ULL + i) << 64) | (0x0901020304050607ULL + i);
    }
#endif

    volatile char* trigger_addr =
        reinterpret_cast<volatile char*>(reinterpret_cast<uintptr_t>(grow_low) - page_size);

#if defined(__x86_64__)
    RegistersValue actual_registers;
    asm volatile(
        "movups   0(%1), %%xmm0\n"
        "movups  16(%1), %%xmm1\n"
        "movups  32(%1), %%xmm2\n"
        "movups  48(%1), %%xmm3\n"
        "movups  64(%1), %%xmm4\n"
        "movups  80(%1), %%xmm5\n"
        "movups  96(%1), %%xmm6\n"
        "movups 112(%1), %%xmm7\n"
        "movups 128(%1), %%xmm8\n"
        "movups 144(%1), %%xmm9\n"
        "movups 160(%1), %%xmm10\n"
        "movups 176(%1), %%xmm11\n"
        "movups 192(%1), %%xmm12\n"
        "movups 208(%1), %%xmm13\n"
        "movups 224(%1), %%xmm14\n"
        "movups 240(%1), %%xmm15\n"

        "movb $42, (%0)\n"

        "movups %%xmm0,   0(%2)\n"
        "movups %%xmm1,  16(%2)\n"
        "movups %%xmm2,  32(%2)\n"
        "movups %%xmm3,  48(%2)\n"
        "movups %%xmm4,  64(%2)\n"
        "movups %%xmm5,  80(%2)\n"
        "movups %%xmm6,  96(%2)\n"
        "movups %%xmm7, 112(%2)\n"
        "movups %%xmm8, 128(%2)\n"
        "movups %%xmm9, 144(%2)\n"
        "movups %%xmm10, 160(%2)\n"
        "movups %%xmm11, 176(%2)\n"
        "movups %%xmm12, 192(%2)\n"
        "movups %%xmm13, 208(%2)\n"
        "movups %%xmm14, 224(%2)\n"
        "movups %%xmm15, 240(%2)\n"
        :
        : "r"(trigger_addr), "r"(&kTestRegisters), "r"(&actual_registers)
        : "memory");
    EXPECT_EQ(actual_registers, kTestRegisters);
#elif defined(__arm__)
    RegistersValue actual_registers;
    asm volatile(
        "vldr d0, [%1, #0]\n"
        "vldr d1, [%1, #8]\n"
        "vldr d2, [%1, #16]\n"
        "vldr d3, [%1, #24]\n"
        "vldr d4, [%1, #32]\n"
        "vldr d5, [%1, #40]\n"
        "vldr d6, [%1, #48]\n"
        "vldr d7, [%1, #56]\n"
        "vldr d8, [%1, #64]\n"
        "vldr d9, [%1, #72]\n"
        "vldr d10, [%1, #80]\n"
        "vldr d11, [%1, #88]\n"
        "vldr d12, [%1, #96]\n"
        "vldr d13, [%1, #104]\n"
        "vldr d14, [%1, #112]\n"
        "vldr d15, [%1, #120]\n"
        "vldr d16, [%1, #128]\n"
        "vldr d17, [%1, #136]\n"
        "vldr d18, [%1, #144]\n"
        "vldr d19, [%1, #152]\n"
        "vldr d20, [%1, #160]\n"
        "vldr d21, [%1, #168]\n"
        "vldr d22, [%1, #176]\n"
        "vldr d23, [%1, #184]\n"
        "vldr d24, [%1, #192]\n"
        "vldr d25, [%1, #200]\n"
        "vldr d26, [%1, #208]\n"
        "vldr d27, [%1, #216]\n"
        "vldr d28, [%1, #224]\n"
        "vldr d29, [%1, #232]\n"
        "vldr d30, [%1, #240]\n"
        "vldr d31, [%1, #248]\n"

        "strb %[val], [%0]\n"

        "vstr d0, [%2, #0]\n"
        "vstr d1, [%2, #8]\n"
        "vstr d2, [%2, #16]\n"
        "vstr d3, [%2, #24]\n"
        "vstr d4, [%2, #32]\n"
        "vstr d5, [%2, #40]\n"
        "vstr d6, [%2, #48]\n"
        "vstr d7, [%2, #56]\n"
        "vstr d8, [%2, #64]\n"
        "vstr d9, [%2, #72]\n"
        "vstr d10, [%2, #80]\n"
        "vstr d11, [%2, #88]\n"
        "vstr d12, [%2, #96]\n"
        "vstr d13, [%2, #104]\n"
        "vstr d14, [%2, #112]\n"
        "vstr d15, [%2, #120]\n"
        "vstr d16, [%2, #128]\n"
        "vstr d17, [%2, #136]\n"
        "vstr d18, [%2, #144]\n"
        "vstr d19, [%2, #152]\n"
        "vstr d20, [%2, #160]\n"
        "vstr d21, [%2, #168]\n"
        "vstr d22, [%2, #176]\n"
        "vstr d23, [%2, #184]\n"
        "vstr d24, [%2, #192]\n"
        "vstr d25, [%2, #200]\n"
        "vstr d26, [%2, #208]\n"
        "vstr d27, [%2, #216]\n"
        "vstr d28, [%2, #224]\n"
        "vstr d29, [%2, #232]\n"
        "vstr d30, [%2, #240]\n"
        "vstr d31, [%2, #248]\n"
        :
        : "r"(trigger_addr), "r"(&kTestRegisters), "r"(&actual_registers), [val] "r"(42)
        : "memory");
    EXPECT_EQ(actual_registers, kTestRegisters);
#elif defined(__aarch64__)
    RegistersValue actual_registers;
    asm volatile(
        "ldr q0, [%1, #0]\n"
        "ldr q1, [%1, #16]\n"
        "ldr q2, [%1, #32]\n"
        "ldr q3, [%1, #48]\n"
        "ldr q4, [%1, #64]\n"
        "ldr q5, [%1, #80]\n"
        "ldr q6, [%1, #96]\n"
        "ldr q7, [%1, #112]\n"
        "ldr q8, [%1, #128]\n"
        "ldr q9, [%1, #144]\n"
        "ldr q10, [%1, #160]\n"
        "ldr q11, [%1, #176]\n"
        "ldr q12, [%1, #192]\n"
        "ldr q13, [%1, #208]\n"
        "ldr q14, [%1, #224]\n"
        "ldr q15, [%1, #240]\n"
        "ldr q16, [%1, #256]\n"
        "ldr q17, [%1, #272]\n"
        "ldr q18, [%1, #288]\n"
        "ldr q19, [%1, #304]\n"
        "ldr q20, [%1, #320]\n"
        "ldr q21, [%1, #336]\n"
        "ldr q22, [%1, #352]\n"
        "ldr q23, [%1, #368]\n"
        "ldr q24, [%1, #384]\n"
        "ldr q25, [%1, #400]\n"
        "ldr q26, [%1, #416]\n"
        "ldr q27, [%1, #432]\n"
        "ldr q28, [%1, #448]\n"
        "ldr q29, [%1, #464]\n"
        "ldr q30, [%1, #480]\n"
        "ldr q31, [%1, #496]\n"

        "strb %w3, [%0]\n"

        "str q0, [%2, #0]\n"
        "str q1, [%2, #16]\n"
        "str q2, [%2, #32]\n"
        "str q3, [%2, #48]\n"
        "str q4, [%2, #64]\n"
        "str q5, [%2, #80]\n"
        "str q6, [%2, #96]\n"
        "str q7, [%2, #112]\n"
        "str q8, [%2, #128]\n"
        "str q9, [%2, #144]\n"
        "str q10, [%2, #160]\n"
        "str q11, [%2, #176]\n"
        "str q12, [%2, #192]\n"
        "str q13, [%2, #208]\n"
        "str q14, [%2, #224]\n"
        "str q15, [%2, #240]\n"
        "str q16, [%2, #256]\n"
        "str q17, [%2, #272]\n"
        "str q18, [%2, #288]\n"
        "str q19, [%2, #304]\n"
        "str q20, [%2, #320]\n"
        "str q21, [%2, #336]\n"
        "str q22, [%2, #352]\n"
        "str q23, [%2, #368]\n"
        "str q24, [%2, #384]\n"
        "str q25, [%2, #400]\n"
        "str q26, [%2, #416]\n"
        "str q27, [%2, #432]\n"
        "str q28, [%2, #448]\n"
        "str q29, [%2, #464]\n"
        "str q30, [%2, #480]\n"
        "str q31, [%2, #496]\n"
        :
        : "r"(trigger_addr), "r"(&kTestRegisters), "r"(&actual_registers), "r"(42)
        : "memory");
    EXPECT_EQ(actual_registers, kTestRegisters);
#elif defined(__riscv)
    RegistersValue actual_registers;
    asm volatile(
        "fld f0, 0(%1)\n"
        "fld f1, 8(%1)\n"
        "fld f2, 16(%1)\n"
        "fld f3, 24(%1)\n"
        "fld f4, 32(%1)\n"
        "fld f5, 40(%1)\n"
        "fld f6, 48(%1)\n"
        "fld f7, 56(%1)\n"
        "fld f8, 64(%1)\n"
        "fld f9, 72(%1)\n"
        "fld f10, 80(%1)\n"
        "fld f11, 88(%1)\n"
        "fld f12, 96(%1)\n"
        "fld f13, 104(%1)\n"
        "fld f14, 112(%1)\n"
        "fld f15, 120(%1)\n"
        "fld f16, 128(%1)\n"
        "fld f17, 136(%1)\n"
        "fld f18, 144(%1)\n"
        "fld f19, 152(%1)\n"
        "fld f20, 160(%1)\n"
        "fld f21, 168(%1)\n"
        "fld f22, 176(%1)\n"
        "fld f23, 184(%1)\n"
        "fld f24, 192(%1)\n"
        "fld f25, 200(%1)\n"
        "fld f26, 208(%1)\n"
        "fld f27, 216(%1)\n"
        "fld f28, 224(%1)\n"
        "fld f29, 232(%1)\n"
        "fld f30, 240(%1)\n"
        "fld f31, 248(%1)\n"
        "addi %[ptr], %[ptr], 256\n"
        "csrr t0, vlenb\n"
        "slli t0, t0, 3\n"
        "vl8r.v v0, (%[ptr])\n"
        "add %[ptr], %[ptr], t0\n"
        "vl8r.v v8, (%[ptr])\n"
        "add %[ptr], %[ptr], t0\n"
        "vl8r.v v16, (%[ptr])\n"
        "add %[ptr], %[ptr], t0\n"
        "vl8r.v v24, (%[ptr])\n"

        "sb %[val], (%0)\n"

        "fsd f0, 0(%2)\n"
        "fsd f1, 8(%2)\n"
        "fsd f2, 16(%2)\n"
        "fsd f3, 24(%2)\n"
        "fsd f4, 32(%2)\n"
        "fsd f5, 40(%2)\n"
        "fsd f6, 48(%2)\n"
        "fsd f7, 56(%2)\n"
        "fsd f8, 64(%2)\n"
        "fsd f9, 72(%2)\n"
        "fsd f10, 80(%2)\n"
        "fsd f11, 88(%2)\n"
        "fsd f12, 96(%2)\n"
        "fsd f13, 104(%2)\n"
        "fsd f14, 112(%2)\n"
        "fsd f15, 120(%2)\n"
        "fsd f16, 128(%2)\n"
        "fsd f17, 136(%2)\n"
        "fsd f18, 144(%2)\n"
        "fsd f19, 152(%2)\n"
        "fsd f20, 160(%2)\n"
        "fsd f21, 168(%2)\n"
        "fsd f22, 176(%2)\n"
        "fsd f23, 184(%2)\n"
        "fsd f24, 192(%2)\n"
        "fsd f25, 200(%2)\n"
        "fsd f26, 208(%2)\n"
        "fsd f27, 216(%2)\n"
        "fsd f28, 224(%2)\n"
        "fsd f29, 232(%2)\n"
        "fsd f30, 240(%2)\n"
        "fsd f31, 248(%2)\n"
        "addi %[out_ptr], %[out_ptr], 256\n"
        "vs8r.v v0, (%[out_ptr])\n"
        "add %[out_ptr], %[out_ptr], t0\n"
        "vs8r.v v8, (%[out_ptr])\n"
        "add %[out_ptr], %[out_ptr], t0\n"
        "vs8r.v v16, (%[out_ptr])\n"
        "add %[out_ptr], %[out_ptr], t0\n"
        "vs8r.v v24, (%[out_ptr])\n"
        :
        : "r"(trigger_addr), [ptr] "r"(&kTestRegisters), [out_ptr] "r"(&actual_registers),
          [val] "r"(42)
        : "memory", "t0");
    EXPECT_EQ(actual_registers, kTestRegisters);
#endif

    // Verify that the mapping grew by trying to map onto trigger_addr with MAP_FIXED_NOREPLACE.
    // It should fail with EEXIST because the page is now mapped.
    void* rv = mmap(const_cast<char*>(trigger_addr), page_size, PROT_READ,
                    MAP_FIXED_NOREPLACE | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    EXPECT_EQ(rv, MAP_FAILED);
    EXPECT_EQ(errno, EEXIST);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

}  // namespace
