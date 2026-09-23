// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_PV_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_PV_H_

#include <lib/page/size.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <ktl/atomic.h>
#include <vm/pmm.h>

static constexpr uint32_t kKvmSystemTimeMsrOld = 0x12;
static constexpr uint32_t kKvmSystemTimeMsr = 0x4b564d01;

static constexpr uint32_t kKvmBootTimeOld = 0x11;
static constexpr uint32_t kKvmBootTime = 0x4b564d00;

static constexpr uint32_t kKvmFeatureClockSourceOld = 1u << 0;
static constexpr uint32_t kKvmFeatureClockSource = 1u << 3;

static constexpr uint8_t kKvmSystemTimeStable = 1u << 0;

// Both structures below are part of the ABI used by Xen and KVM, this ABI is not
// defined by us we just follow it. For more detail please refer to the
// documentation (https://www.kernel.org/doc/Documentation/virtual/kvm/msr.txt).
struct pv_clock_boot_time {
  // With multiple VCPUs it is possible that one VCPU can try to read boot time
  // while we are updating it because another VCPU asked for the update. In this
  // case odd version value serves as an indicator for the guest that update is
  // in progress. Therefore we need to update version before we write anything
  // else and after, also we need to user proper memory barriers. The same logic
  // applies to system time version below, even though system time is per VCPU
  // others VCPUs still can access system times of other VCPUs (Linux however
  // never does that).
  uint32_t version;
  uint32_t seconds;
  uint32_t nseconds;
};
static_assert(sizeof(struct pv_clock_boot_time) == 12, "sizeof(pv_clock_boot_time) should be 12");

struct pv_clock_system_time {
  uint32_t version;
  uint32_t pad0;
  uint64_t tsc_timestamp;
  uint64_t system_time;
  uint32_t tsc_mul;
  int8_t tsc_shift;
  uint8_t flags;
  uint8_t pad1[2];
};

extern "C" {

// Initialize the para-virtualized clock.
//
// This function should only be called by CPU 0.
zx_status_t rust_pv_clock_init();

// Initialize all PvEoi instances.
//
// Must be called from a context in which blocking is allowed.
void rust_pveoi_init_all();

// Enable PV_EOI for the current CPU. After it is enabled, callers may use Eoi() rather than
// access a local APIC register if desired.
//
// Explicitly uses real msr accesses and is not suitable for unit testing.
//
// Once enabled this PvEoi object must be disabled prior to destruction.
//
// It is an error to enable a PvEoi object more than once over its lifetime.
void rust_pveoi_enable_real_msr();
// Attempt to acknowledge and signal an end-of-interrupt (EOI) for the current CPU via a
// paravirtual interface. If a fast acknowledge was not available, the function returns
// false and the caller must signal an EOI via the legacy mechanism.
bool rust_pveoi_eoi();

// Shutsdown the para-virtualized clock.
//
// This function should only be called by CPU 0.
void pv_clock_shutdown();

bool pv_clock_is_stable();
uint64_t pv_clock_get_tsc_freq();

// Send para-virtualized IPI.
//
// @param mask_low Low part of CPU mask.
// @param mask_high High part of CPU mask.
// @param start_id APIC ID that the CPU mask starts at.
// @param icr APIC ICR value.
// @return The number of CPUs that the IPI was delivered to, or an error value.
int pv_ipi(uint64_t mask_low, uint64_t mask_high, uint64_t start_id, uint64_t icr);
}

// Initialize the para-virtualized clock.
//
// This function should only be called by CPU 0.
inline zx_status_t pv_clock_init() { return rust_pv_clock_init(); }

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_PV_H_
