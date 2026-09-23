// Copyright 2017 The Fuchsia Authors
// Copyright (c) 2016, Google, Inc. All rights reserved
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_PSCI_INCLUDE_DEV_PSCI_H_
#define ZIRCON_KERNEL_DEV_PSCI_INCLUDE_DEV_PSCI_H_

// Known PSCI calls as of PSCI 1.2, Document DEN0022E
#define PSCI64_PSCI_VERSION (0x84000000)
#define PSCI64_CPU_SUSPEND (0xC4000001)
#define PSCI64_CPU_OFF (0x84000002)
#define PSCI64_CPU_ON (0xC4000003)
#define PSCI64_AFFINITY_INFO (0xC4000004)
#define PSCI64_MIGRATE (0xC4000005)
#define PSCI64_MIGRATE_INFO_TYPE (0x84000006)
#define PSCI64_MIGRATE_INFO_UP_CPU (0xC4000007)
#define PSCI64_SYSTEM_OFF (0x84000008)
#define PSCI64_SYSTEM_RESET (0x84000009)
#define PSCI64_SYSTEM_RESET2 (0xC4000012)
#define PSCI64_PSCI_FEATURES (0x8400000A)
#define PSCI64_CPU_FREEZE (0x8400000B)
#define PSCI64_CPU_DEFAULT_SUSPEND (0xC400000C)
#define PSCI64_NODE_HW_STATE (0xC400000D)
#define PSCI64_SYSTEM_SUSPEND (0xC400000E)
#define PSCI64_PSCI_SET_SUSPEND_MODE (0x8400000F)
#define PSCI64_PSCI_STAT_RESIDENCY (0xC4000010)
#define PSCI64_PSCI_STAT_COUNT (0xC4000011)
#define PSCI64_MEM_PROTECT (0x84000013)
#define PSCI64_MEM_PROTECT_RANGE (0xC4000014)

// See: "Firmware interfaces for mitigating cache speculation vulnerabilities"
//      "System Software on Arm Systems"
#define PSCI64_SMCCC_VERSION (0x80000000)
#define PSCI64_SMCCC_ARCH_FEATURES (0x80000001)
#define PSCI64_SMCCC_ARCH_WORKAROUND_1 (0x80008000)
#define PSCI64_SMCCC_ARCH_WORKAROUND_2 (0x80007FFF)

// See section 5.2.2 of "Arm Power State Coordination Interface", DEN0022F.b.
#define PSCI_SUCCESS 0
#define PSCI_NOT_SUPPORTED -1
#define PSCI_INVALID_PARAMETERS -2
#define PSCI_DENIED -3
#define PSCI_ALREADY_ON -4
#define PSCI_ON_PENDING -5
#define PSCI_INTERNAL_FAILURE -6
#define PSCI_NOT_PRESENT -7
#define PSCI_DISABLED -8
#define PSCI_INVALID_ADDRESS -9

#ifndef __ASSEMBLER__

#include <arch.h>
#include <lib/zbi-format/driver-config.h>

#include <arch/arm64/mp.h>
#include <dev/power.h>
#include <ktl/span.h>

/* TODO NOTE: - currently these routines assume cpu topologies that are described only in AFF0 and
   AFF1. If a system is architected such that AFF2 or AFF3 are non-zero then this code will need to
   be revisited
*/

// Initializes the PSCI driver.
void PsciInit(const zbi_dcfg_arm_psci_driver_t& config,
              ktl::span<const zbi_dcfg_arm_psci_cpu_suspend_state_t> psci_cpu_suspend_config);

uint32_t psci_get_version();
uint32_t psci_get_feature(uint32_t psci_call);

#include <zircon/compiler.h>

__BEGIN_CDECLS

/* powers down the calling cpu - only returns if call fails */
zx_status_t psci_cpu_off();
zx_status_t psci_cpu_on(uint64_t mpid, paddr_t entry, uint64_t context);

__END_CDECLS

// Specifies whether a requested power state can target more than the calling
// CPU.
enum class PsciCpuSuspendMaxScope : bool { CpuOnly, CpuAndMore };

// Returns an appropriate power_state value to be used with |psci_cpu_suspend|.
//
// |scope| specifies the maximum topological scope that the selected power state
// is allowed to target.
//
// |last| indicates whether the calling CPU is the last one to enter suspend.
uint32_t psci_select_cpu_suspend_power_state(PsciCpuSuspendMaxScope scope, bool last);

// Returns true if a |psci_cpu_suspend| might result in a powerdown event for
// the calling CPU.  This function allows callers to skip saving/restoring some
// CPU state in the case where the suspend operation will not powerdown the
// calling CPU.
bool psci_might_powerdown();

// Whether or not the CPU powered down (lost state) during a CPU_SUSPEND
// operation.  See |psci_cpu_suspend|.
enum class CpuPoweredDown : bool { No, Yes };
using PsciCpuSuspendResult = zx::result<CpuPoweredDown>;

// Enters a PSCI CPU_SUSPEND state on the calling CPU.
//
// |power_state| specifies the power state to enter.  See also
// |psci_select_cpu_suspend_power_state|.
//
// Prior to calling, interrupts must be disabled, preemption must be disabled,
// and the caller must be pinned to the calling CPU.
//
// The calling thread must be a kernel-only thread.  Because this routine saves
// only the minimum required register state, it is an error to call this
// function on a thread that has a "user half" (ThreadDispatcher).
//
// On success, returns ZX_OK with either CpuPoweredDown::No or
// CpuPoweredDown::Yes, depending on whether the CPU actually powered down
// during the operation.
//
// Returns ZX_ERR_NOT_SUPPORTED if CPU_SUSPEND is not supported on this platform.
//
// Returns ZX_ERR_INVALID_PARAMETERS if the requested power_state is invalid, or
// a low-power state was requested for a higher-than-core-level topology node
// (e.g. cluster) and at least one of the children in that node is in a local
// low-power state that is incompatible with the request.  This is a "normal"
// error that callers must be prepared to handle.
//
// Returns ZX_ERR_ACCESS_DENIED if a low-power state was requested for a
// higher-than-core-level topology node (e.g. cluster) and all the cores that
// are in an incompatible state with the request are running, as opposed to
// being in a low-power state.  This is a "normal" error that callers must be
// prepared to handle (typically with a retry).
PsciCpuSuspendResult psci_cpu_suspend(uint32_t power_state);

// Holds register state to be saved/restored across a suspend/resume cycle.
//
// Must be kept in sync with |psci_do_suspend| and |psci_do_resume|.
struct psci_cpu_resume_context {
  uint64_t regs[2 * 14]{};
};

__BEGIN_CDECLS

int64_t psci_get_affinity_info(uint64_t mpid);
zx_status_t psci_get_cpu_state(uint64_t mpid, power_cpu_state* out_state);
zx_status_t psci_system_off();
zx_status_t psci_system_reset(power_reboot_flags flags);
zx_status_t psci_system_reset_cold();

// Used when calling SYSTEM_RESET2 directly
zx_status_t psci_system_reset2_raw(uint32_t reset_type, uint32_t cookie);

__END_CDECLS

enum psci_suspend_mode : uint32_t { platform_coordinated = 0, os_initiated = 1 };
zx_status_t psci_set_suspend_mode(psci_suspend_mode mode);

// Returns true iff PSCI version is 1.0 or better and SET_SUSPEND_MODE is supported.
bool psci_is_set_suspend_mode_supported();

// Returns true iff PSCI version is 1.0 or better, and CPU_SUSPEND is supported,
// and we have valid ZBI-supplied power_state configuration.
bool psci_is_cpu_suspend_supported();

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_DEV_PSCI_INCLUDE_DEV_PSCI_H_
