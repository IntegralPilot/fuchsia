// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MWAIT_MONITOR_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MWAIT_MONITOR_H_

#include <assert.h>
#include <debug.h>
#include <lib/arch/x86/boot-cpuid.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/x86.h>
#include <fbl/alloc_checker.h>
#include <fbl/macros.h>
#include <kernel/cpu.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/memory.h>
#include <ktl/new.h>
#include <ktl/type_traits.h>

// MwaitMonitor is used to perform MWAIT/MONITOR signaling.
//
// This is a thread-safe type.
class MwaitMonitor {
 public:
  // Atomically replace this monitor's value |value| and return the old value.
  uint8_t Exchange(uint8_t value) { return m_.exchange(value, ktl::memory_order_relaxed); }

  // Atomically write |value| to this monitor.
  void Write(uint8_t value) { m_.store(value, ktl::memory_order_relaxed); }

  // Atomically read the monitor and return its value.
  uint8_t Read() const { return m_.load(ktl::memory_order_relaxed); }

  // Tells the hardware to being watching this monitor.  See |x86_enable_ints_and_mwait|.
  void PrepareForWait() { x86_monitor(&m_); }

 private:
  ktl::atomic<uint8_t> m_{};
};

// An array of mwait/monitor objects with optimal alignment.
class MwaitMonitorArray {
 public:
  MwaitMonitorArray() = default;
  ~MwaitMonitorArray() {
    delete[] monitors_;
    monitors_ = nullptr;
  }
  DISALLOW_COPY_ASSIGN_AND_MOVE(MwaitMonitorArray);

  // Initialize this instance with enough monitors for |cpu_count| CPUs.
  //
  // It is an error to successfully initialize a given instance more than once.
  zx_status_t Init(size_t cpu_count) {
    DEBUG_ASSERT(monitors_ == nullptr);

    // Each CPU needs one monitor.  However, we want to ensure that each one is aligned by the max
    // cache line size or the monitor line size (whichever is larger) so we allocate more than we
    // need and just use ones that meet our alignment goals.
    const size_t alignment =
        ktl::max(arch::BootCpuid<arch::CpuidMonitorMwaitB>().largest_monitor_line_size(),
                 static_cast<unsigned int>(MAX_CACHE_LINE));
    const size_t size = alignment * cpu_count;
    static_assert(sizeof(MwaitMonitor) == 1);
    static_assert(alignof(MwaitMonitor) == 1);

    fbl::AllocChecker ac;
    MwaitMonitor* monitors = new (ktl::align_val_t{alignment}, ac) MwaitMonitor[size];
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }

    monitors_ = monitors;
    alignment_ = alignment;
    return ZX_OK;
  }

  // Return a mutable reference to the monitor for |cpu_num|.
  //
  // Note, the monitors held by this array will be destroyed when this array is destroyed.
  MwaitMonitor& GetForCpu(cpu_num_t cpu_num) {
    DEBUG_ASSERT(monitors_ != nullptr);
    return monitors_[cpu_num * alignment_];
  }

 private:
  MwaitMonitor* monitors_{};
  size_t alignment_{};
};

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MWAIT_MONITOR_H_
