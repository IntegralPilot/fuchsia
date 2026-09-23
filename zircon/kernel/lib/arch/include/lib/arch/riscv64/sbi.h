// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SBI_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SBI_H_

// RISC-V Supervisor Binary Interface Specification
//
// https://github.com/riscv-non-isa/riscv-sbi-doc/blob/master/riscv-sbi.adoc

#include <cstdint>

namespace arch {

// This provides enums and types for the SBI ABI.
// See <lib/arch/riscv64/sbi-call.h> for functions.

enum class RiscvSbiError : intptr_t {
  kSuccess = 0,
  kFailed = -1,
  kNotSupported = -2,
  kInvalidParam = -3,
  kDenied = -4,
  kInvalidAddress = -5,
  kAlreadyAvailable = -6,
  kAlreadyStarted = -7,
  kAlreadyStopped = -8,
  kNoShmem = -9,
};

struct RiscvSbiRet {
  RiscvSbiError error;
  intptr_t value;
};

enum class RiscvSbiEid : uint32_t {
  kBase = 0x10,
  kTimer = 0x54494d45,        // 'TIME'
  kIpi = 0x735049,            // 'sPI'
  kRfence = 0x52464e43,       // 'RFNC'
  kHart = 0x48534d,           // 'HSM'
  kSystemReset = 0x53525354,  // 'SRST'
  kPmu = 0x504D55,            // 'PMU'
  kDbcn = 0x4442434e,         // 'DBCN'
  kSusp = 0x53555350,         // 'SUSP'
  kCppc = 0x43505043,         // 'CPPC'

  kFirstExperimental = 0x08000000,
  kLastExperimental = 0x08ffffff,

  kFirstVendor = 0x09000000,
  kLastVendor = 0x09ffffff,

  kFirstFirmware = 0x0a000000,
  kLastFirmware = 0x0affffff,
};

// Base Extension (RiscvSbiEid::kBase) FIDs
enum class RiscvSbiBase : uint32_t {
  kGetSpecVersion = 0,
  kGetImplId = 1,
  kGetImplVersion = 2,
  kProbeExtension = 3,
  kGetMvendorid = 4,
  kGetMarchid = 5,
  kGetMimpid = 6,
};

// Timer Extension (RiscvSbiEid::kTimer) FIDs
enum class RiscvSbiTimer : uint32_t {
  kSetTimer = 0,
};

// IPI Extension (RiscvSbiEid::kIpi) FIDs
enum class RiscvSbiIpi : uint32_t {
  kSendIpi = 0,
};

// Remote Fence Extension (RiscvSbiEid::kRfence) FIDs
enum class RiscvSbiRfence : uint32_t {
  kFenceI = 0,
  kSfenceVma = 1,
  kSfenceVmaAsid = 2,
  kHfenceGvmaVid = 3,
  kHfenceGvma = 4,
  kHfenceVvmaAsid = 5,
  kHfenceVvma = 6,
};

// Hart State Management Extension (RiscvSbiEid::kHart) FIDs
enum class RiscvSbiHart : uint32_t {
  kStart = 0,
  kStop = 1,
  kGetStatus = 2,
  kSuspend = 3,
};

enum class RiscvSbiHartState : uint32_t {
  kStarted = 0,
  kStopped = 1,
  kStartPending = 2,
  kStopPending = 3,
  kSuspended = 4,
  kSuspendPending = 5,
  kResumePending = 6,
};

// System Reset Extension (RiscvSbiEid::kSystemReset) FIDs
enum class RiscvSbiSystemReset : uint32_t {
  kSystemReset = 0,
};

enum class RiscvSbiResetType : uint32_t {
  kShutdown = 0,
  kColdReboot = 1,
  kWarmReboot = 2,

  kFirstReserved = 3,
  kLastReserved = 0xefffffff,

  kFirstVendor = 0xf0000000,
  kLastVendor = 0xffffffff,
};

enum class RiscvSbiResetReason : uint32_t {
  kNone = 0,
  kSystemFailure = 1,

  kFirstReserved = 2,
  kLastReserved = 0xdfffffff,

  kFirstSbiImpl = 0xe0000000,
  kLastSbiImpl = 0xefffffff,

  kFirstVendor = 0xf0000000,
  kLastVendor = 0xffffffff,
};

using HartId = uint64_t;
using HartMask = uintptr_t;
using HartMaskBase = uintptr_t;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_SBI_H_
