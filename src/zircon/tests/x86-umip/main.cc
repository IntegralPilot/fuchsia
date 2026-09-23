// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cpuid.h>
#include <lib/arch/x86/descriptor.h>
#include <lib/captive-thread/captive-thread.h>
#include <lib/captive-thread/testing/matchers.h>

#include <optional>
#include <string>

#include <gtest/gtest.h>

namespace {

using namespace std::literals;

using captive_thread::CaptiveThread;
using captive_thread::testing::GotException;
using captive_thread::testing::WithType;
using ::testing::Not;

class X86UmipTests : public ::testing::Test {
 public:
  static void SetUpTestSuite() {
    uint32_t values[4];
    is_umip_supported_ =
        __get_cpuid_count(7, 0, &values[0], &values[1], &values[2], &values[3]) == 1 &&
        (values[2] & (1u << 2));

    __cpuid(0x40000000, values[0], values[1], values[2], values[3]);
    std::string_view name{reinterpret_cast<const char*>(&values[1]), 12};
    on_kvm_ = name == "KVMKVMKVM\0\0\0"sv;
  }

  static bool IsUmipSupported() { return *is_umip_supported_; }

  static bool OnKvm() { return *on_kvm_; }

 private:
  static inline std::optional<bool> is_umip_supported_ = std::nullopt;
  static inline std::optional<bool> on_kvm_ = std::nullopt;
};

constexpr auto NoException() { return Not(GotException()); }

constexpr auto GotGpf() { return GotException(WithType(ZX_EXCP_GENERAL)); }

// Turn a string literal into an array that can be a template parameter.
template <size_t N>
consteval auto Literal(const char (&str)[N]) {
  std::array<char, N - 1> out;
  std::string_view{str, N - 1}.copy(out.data(), out.size());
  return out;
}

// Attempt the instruction that should write something to its output memory
// operand without UMIP and #GPF instead with UMIP.
template <auto Insn, typename T = arch::GdtRegister64>
void StoreInsn() {
  T out;
  constexpr std::string tmpl = std::string(Insn.data(), Insn.size()) + " %0";
  __asm__ volatile((tmpl) : "=m"(out));
}

TEST_F(X86UmipTests, Noop) {
  CaptiveThread thread([] { __asm__ volatile("nop"); });
  EXPECT_THAT(thread.WaitForException(), NoException());
}

TEST_F(X86UmipTests, Noncanonical) {
  CaptiveThread thread([] {
    constexpr uint64_t kNoncanonical = uint64_t{1} << 63;
    volatile uint64_t* ptr;
    // The compiler doesn't know what the ptr value is, so it can't remove the
    // volatile store.
    __asm__("" : "=r"(ptr) : "0"(kNoncanonical));
    *ptr = 0;
  });
  EXPECT_THAT(thread.WaitForException(), GotGpf());
}

TEST_F(X86UmipTests, Sgdt) {
  CaptiveThread thread(StoreInsn<Literal("sgdt")>);
  if (IsUmipSupported()) {
    EXPECT_THAT(thread.WaitForException(), GotGpf());
  } else {
    EXPECT_THAT(thread.WaitForException(), NoException());
  }
}

TEST_F(X86UmipTests, Sidt) {
  CaptiveThread thread(StoreInsn<Literal("sidt")>);
  if (IsUmipSupported()) {
    EXPECT_THAT(thread.WaitForException(), GotGpf());
  } else {
    EXPECT_THAT(thread.WaitForException(), NoException());
  }
}

TEST_F(X86UmipTests, Sldt) {
  CaptiveThread thread(StoreInsn<Literal("sldt")>);
  if (IsUmipSupported()) {
    EXPECT_THAT(thread.WaitForException(), GotGpf());
  } else {
    EXPECT_THAT(thread.WaitForException(), NoException());
  }
}

TEST_F(X86UmipTests, Str) {
  CaptiveThread thread(StoreInsn<Literal("str")>);
  if (IsUmipSupported()) {
    EXPECT_THAT(thread.WaitForException(), GotGpf());
  } else {
    EXPECT_THAT(thread.WaitForException(), NoException());
  }
}

TEST_F(X86UmipTests, Smsw) {
  CaptiveThread thread(StoreInsn<Literal("smsw"), uint16_t>);
  if (IsUmipSupported()) {
    // If UMIP is supported, check if we're running under KVM.  On host
    // hardware that does not support UMIP, KVM misemulates UMIP's effect on
    // the SMSW instruction.
    if (OnKvm()) {
      GTEST_SKIP() << "KVM may misemulate SMSW instruction under UMIP";
    }
    EXPECT_THAT(thread.WaitForException(), GotGpf());
  } else {
    EXPECT_THAT(thread.WaitForException(), NoException());
  }
}

}  // namespace
