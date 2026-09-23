// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/features.h>
#include <zircon/syscalls.h>

#include <zxtest/zxtest.h>

namespace {

#ifdef __aarch64__

TEST(CpuFeatureTests, ArmMops) {
  uint32_t features = 0;
  ASSERT_OK(zx_system_get_features(ZX_FEATURE_KIND_CPU, &features));

  if ((features & ZX_ARM64_FEATURE_ISA_MOPS) == 0) {
    ZXTEST_SKIP("FEAT_MOPS not detected");
    return;
  }

  uint64_t mem = 0xaaaaaaaaaaaaaaaa;
  uint64_t* dst = &mem;
  uint64_t count = sizeof(mem);
  uint64_t byte = 0xbb;
  __asm__(
      R"""(
      .arch_extension mops
      setp [%[dst]]!, %[count]!, %[byte]
      setm [%[dst]]!, %[count]!, %[byte]
      sete [%[dst]]!, %[count]!, %[byte]
      )"""
      // The memory output operand tells the compiler that the memory will be
      // touched, so it can't presume it knows the value.  It also being an
      // input via `+` tells the compiler that the initialization store is not
      // dead, so we're sure the memory had the old value before the SETP.
      : "+m"(*dst), [dst] "+r"(dst), [count] "+r"(count)
      : [byte] "r"(byte)
      : "cc");
  EXPECT_EQ(mem, 0xbbbbbbbbbbbbbbbb);
}

TEST(CpuFeatureTests, ArmMopsException) {
  uint32_t features = 0;
  ASSERT_OK(zx_system_get_features(ZX_FEATURE_KIND_CPU, &features));

  if ((features & ZX_ARM64_FEATURE_ISA_MOPS) == 0) {
    ZXTEST_SKIP("FEAT_MOPS not detected");
    return;
  }

  // The prologue insn sets or clears the C flag to indicate its algorithm.
  // The main body insn checks that C matches its algorithm, but is only
  // required to check when it has work to do.  There's no guarantee how much
  // the prologue insn can do by itself, but a very large count (greater than
  // page size) effectively ensures that the main body will have work to do and
  // so will be required to check C and raise the exception.  There's no
  // guarantee whether C will be set or cleared, but it must be left as is for
  // the main body insn.  So simply inverting C between the two instructions
  // should produce the exception.  It's technically not quite kosher to use
  // the instructions in any way other than precisely in the adjacent sequence
  // of three with no intervening instructions.  But this probably has the
  // intended effect on real CPUs as well as emulators, and is certainly easier
  // than rigging things up to single-step and invert the flag via exception
  // return without breaking the prescribed adjacency of the instructions.
  constexpr auto bad_mops = [] {
    std::array<char, 0x20000> mem;
    memset(&mem, 0xaa, sizeof(mem));
    auto* dst = &mem;
    uint64_t count = sizeof(mem);
    uint64_t byte = 0xbb;
    uint64_t scratch;
    __asm__ volatile(
        R"""(
        .arch_extension mops
        setp [%[dst]]!, %[count]!, %[byte]
        mrs %[flags], nzcv
        eor %[flags], %[flags], %[c]
        msr nzcv, %[flags]
        setm [%[dst]]!, %[count]!, %[byte]
        )"""
        : "+m"(*dst), [dst] "+r"(dst), [count] "+r"(count), [flags] "=&r"(scratch)
        : [byte] "r"(byte), [c] "i"(1 << 29)
        : "cc");
  };
  ASSERT_DEATH(bad_mops);
}

#endif

}  // namespace
