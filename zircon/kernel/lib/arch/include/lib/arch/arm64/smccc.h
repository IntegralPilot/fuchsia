// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SMCCC_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SMCCC_H_

#include <stddef.h>
#include <stdint.h>

#if __cpp_impl_three_way_comparison >= 201907L
#include <compare>
#endif

namespace arch {

// [arm/smccc] SMCCC is the SMC (Secure Monitor Call) Calling Convention.
//
// The 32-bit Function identifier is passed in w0, with arguments in
// consecutive registers from x1/w1 up.  Each different function uses a
// specified number of argument registers and a specified number of return
// value registers.
//
// The SMC implementation is required to preserve all registers except for
// x0, x1, x2, and x3, which may be clobbered by any SMC call.  Most calls
// don't use more than those for arguments or return values.  Specific
// calls may be documented to use more registers for return values.

// [arm/smccc] 2.5.3: Conduits
// An SMCCC call is invoked using one of two "conduit" instructions:
enum class ArmSmcccConduit : uint32_t {
  kSmc = 0xd4000003,  // smc #0
  kHvc = 0xd4000002,  // hvc #0
};

// [arm/smccc] 2.5: Function Identifiers
// Each function is assigned a specific 32-bit value, but some bits have
// uniform meaning across all assigned function identifiers.

// This bit is set in all "fast" (vs "yielding") calls.
inline constexpr uint32_t kArmSmcccFunctionFast = 0x8000'0000;

// If this bit is set, the call uses 64-bit argument and result values.
// These calls can use registers up to x17 for results, while the 32-bit
// calls use only up to x7.
inline constexpr uint32_t kArmSmcccFunction64 = 0x4000'0000;

// [arm/smccc] 6: Function Identifier Ranges
//
// 0x8000'0000 - 0x8000'ffff: Arm Architecture Calls
// 0x8100'0000 - 0x8100'ffff: CPU Service Calls
// 0x8200'0000 - 0x8200'ffff: SIP Service Calls
// 0x8300'0000 - 0x8300'ffff: OEM Service Calls
// 0x8400'0000 - 0x8400'ffff: Standard Secure Service Calls
enum class ArmSmcccFunction : uint32_t {
  // [arm/smccc] 7: Arm Architecture Calls
  kSmcccVersion = 0x8000'0000,
  kSmcccArchFeatures = 0x8000'0001,
  kSmcccArchSocId = 0x8000'0002,
  kSmcccArchWorkaround1 = 0x8000'8000,
  kSmcccArchWorkaround2 = 0x8000'7fff,
  kSmcccArchWorkaround3 = 0x8000'3fff,

  // [arm/psci] 5.1: Function prototypes
  // 0x8400'0000 - 0x8400'001f and their SMC64 counterparts at
  // 0xc400'0000 - 0xc400'001f are reserved by [arm/smccc] for PSCI.
  //
  // These are the SMC64 versions when both SMC32 and SMC64 versions are
  // defined.
  kPsciPsciVersion = 0x8400'0000,
  kPsciCpuSuspend = 0xc400'0001,
  kPsciCpuOff = 0x8400'0002,
  kPsciCpuOn = 0xc400'0003,
  kPsciAffinityInfo = 0xc400'0004,
  kPsciMigrate = 0xc400'0005,
  kPsciMigrateInfoType = 0x8400'0006,
  kPsciMigrateInfoUpCpu = 0xc400'0007,
  kPsciSystemOff = 0x8400'0008,
  kPsciSystemReset = 0x8400'0009,
  kPsciSystemReset2 = 0xc400'0012,
  kPsciMemProtect = 0x8400'0013,
  kPsciMemProtectCheckRange = 0xc400'0014,
  kPsciPsciFeatures = 0x8400'000a,
  kPsciCpuFreeze = 0x8400'000b,
  kPsciCpuDefaultSuspend = 0x8400'000c,
  kPsciNodeHwState = 0x8400'000d,
  kPsciSystemSuspend = 0xc400'000e,
  kPsciPsciSetSuspendMode = 0x8400'000f,
  kPsciPsciStatResidency = 0xc400'0010,
  kPsciPsciStatCount = 0xc400'0011,
};

// [arm/smccc] 7.1: Return Codes
// These are the standard values for the Arm Architecture Calls' return
// values in w0.  Some calls return other values, hence this does not use
// a scoped enum.
enum ArmSmcccReturnCode : uint32_t {
  kArmSmcccSuccess = 0,
  kArmSmcccNotSupported = -1u,
  kArmSmcccNotRequired = -2u,
  kArmSmcccInvalidParameter = -3u,
};

// This takes a 64-bit result in x0 and gets the 32-bit standard return code.
constexpr ArmSmcccReturnCode ArmSmcccGetReturnCode(uint64_t x0) {
  return static_cast<ArmSmcccReturnCode>(static_cast<uint32_t>(x0));
}

// Both SMCCC_VERSION and PSCI_VERSION calls return a major/minor version.
struct ArmSmcccVersion {
  constexpr uint32_t ComparisonValue() const {
    return (static_cast<uint32_t>(major) << 16) | static_cast<uint32_t>(minor);
  }

  constexpr bool operator==(const ArmSmcccVersion& other) const {
    return ComparisonValue() == other.ComparisonValue();
  }

  constexpr bool operator!=(const ArmSmcccVersion& other) const {
    return ComparisonValue() != other.ComparisonValue();
  }

#if __cpp_impl_three_way_comparison >= 201907L

  constexpr auto operator<=>(const ArmSmcccVersion& other) const {
    return ComparisonValue() <=> other.ComparisonValue();
  }

#else  // No operator<=>.

  constexpr bool operator<(const ArmSmcccVersion& other) const {
    return ComparisonValue() < other.ComparisonValue();
  }

  constexpr bool operator<=(const ArmSmcccVersion& other) const {
    return ComparisonValue() <= other.ComparisonValue();
  }

  constexpr bool operator>(const ArmSmcccVersion& other) const {
    return ComparisonValue() > other.ComparisonValue();
  }

  constexpr bool operator>=(const ArmSmcccVersion& other) const {
    return ComparisonValue() >= other.ComparisonValue();
  }

#endif  // operator<=>

  uint16_t major = 0, minor = 0;
};

// [arm/smccc] 7.2: SMCCC_VERSION
// This interprets the return value from SMCCC_VERSION or PSCI_VERSION.
constexpr ArmSmcccVersion ArmSmcccVersionResult(  //
    uint64_t x0, ArmSmcccVersion not_supported = {1, 0}) {
  int32_t result = static_cast<int32_t>(x0);
  if (result < 0) {  // Presumably kArmSmcccNotSupported.
    // The call is optional in v1.0, so treat NOT_SUPPORTED as being v1.0.
    return not_supported;
  }
  return {
      .major = static_cast<uint16_t>(result >> 16),
      .minor = static_cast<uint16_t>(result & 0xffff),
  };
}

// [arm/psci] 5.2.1: Register usage in arguments and return values
// The function ID is passed in x0, with arguments in x1, x2, and x3.
// A value is returned in x0.
inline constexpr size_t kArmPsciRegisters = 4;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_SMCCC_H_
