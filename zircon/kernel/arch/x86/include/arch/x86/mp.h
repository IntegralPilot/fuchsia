// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2016 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MP_H_
#define ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MP_H_

// describes the per cpu structure pointed to by gs: in the kernel

// offsets into this structure, used by assembly
#define PERCPU_DIRECT_OFFSET 0x0
#define PERCPU_CURRENT_THREAD_OFFSET 0x8
//      ZX_TLS_STACK_GUARD_OFFSET      0x10
//      ZX_TLS_UNSAFE_SP_OFFSET        0x18
#define PERCPU_SAVED_USER_SP_OFFSET 0x20
#define PERCPU_IN_RESTRICTED_MODE 0x28
#define PERCPU_GPF_RETURN_OFFSET 0x50
#define PERCPU_CPU_NUM_OFFSET 0x58
#define PERCPU_HIGH_LEVEL_PERCPU_OFFSET 0x68
#define PERCPU_DEFAULT_TSS_OFFSET 0x70
#define PERCPU_INTERRUPT_STACKS_NMI_OFFSET 0x20e0

// offset of default_tss.rsp0
#define PERCPU_KERNEL_SP_OFFSET (PERCPU_DEFAULT_TSS_OFFSET + 4)

#define INTERRUPT_STACK_SIZE (4096)

#ifndef __ASSEMBLER__

#include <align.h>
#include <assert.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/tls.h>
#include <zircon/types.h>

#include <arch/x86.h>
#include <arch/x86/idle_states.h>
#include <arch/x86/idt.h>
#include <kernel/cpu.h>
#include <ktl/atomic.h>
#include <ktl/declval.h>
#include <ktl/type_traits.h>

struct Thread;
struct percpu;
class MwaitMonitor;

// This struct is accessed very early during boot so it's important that it can live in the BSS and
// be initialized without calling global constructors.
struct x86_percpu {
  // a direct pointer to ourselves
  x86_percpu* direct = nullptr;

  // the current thread
  Thread* current_thread = nullptr;

  // The offsets of these two slots are published in
  // system/public/zircon/tls.h and known to the compiler.
  uintptr_t stack_guard = 0;
  uintptr_t kernel_unsafe_sp = 0;

  // temporarily saved during a syscall
  uintptr_t saved_user_sp = 0;

  // flag to track that we're in restricted mode
  uint32_t in_restricted_mode = 0;

  // Whether blocking is disallowed.  See arch_blocking_disallowed().
  uint32_t blocking_disallowed = 0;

  // Used for IPI-free rescheduling of idle CPUs with monitor/mwait.
  MwaitMonitor* monitor = 0;

  // Interlock to avoid HLT on idle CPUs without monitor/mwait.
  // halt_interlock is never used on CPUs that have enabled monitor/mwait for idle.
  ktl::atomic<uint32_t> halt_interlock;

  // Supported mwait C-states for idle CPUs.
  X86IdleStates* idle_states = nullptr;

  // local APIC id
  uint32_t apic_id = 0;

  // If nonzero and we receive a GPF, change the return IP to this value.
  uintptr_t gpf_return_target = 0;

  // CPU number
  cpu_num_t cpu_num = 0;

  // Number of spinlocks currently held
  uint32_t num_spinlocks = 0;

  // Last user VmAspace that was active on this core. Lazily updated.
  void* last_user_aspace = nullptr;

  // A pointer providing fast access to the high-level arch-agnostic per-cpu struct.
  percpu* high_level_percpu = nullptr;

  // This CPU's default TSS
  tss_t default_tss __ALIGNED(16) = {};

  // Reserved space for special interrupt stacks
  struct {
    uint8_t nmi[INTERRUPT_STACK_SIZE] __ALIGNED(16);
    uint8_t machine_check[INTERRUPT_STACK_SIZE] __ALIGNED(16);
    uint8_t double_fault[INTERRUPT_STACK_SIZE] __ALIGNED(16);
  } interrupt_stacks = {};
} __CPU_ALIGN;

static_assert(__offsetof(x86_percpu, direct) == PERCPU_DIRECT_OFFSET);
static_assert(__offsetof(x86_percpu, current_thread) == PERCPU_CURRENT_THREAD_OFFSET);
static_assert(__offsetof(x86_percpu, stack_guard) == ZX_TLS_STACK_GUARD_OFFSET);
static_assert(__offsetof(x86_percpu, kernel_unsafe_sp) == ZX_TLS_UNSAFE_SP_OFFSET);
static_assert(__offsetof(x86_percpu, saved_user_sp) == PERCPU_SAVED_USER_SP_OFFSET);
static_assert(__offsetof(x86_percpu, gpf_return_target) == PERCPU_GPF_RETURN_OFFSET);
static_assert(__offsetof(x86_percpu, cpu_num) == PERCPU_CPU_NUM_OFFSET);
static_assert(__offsetof(x86_percpu, high_level_percpu) == PERCPU_HIGH_LEVEL_PERCPU_OFFSET);
static_assert(__offsetof(x86_percpu, default_tss) == PERCPU_DEFAULT_TSS_OFFSET);
static_assert(__offsetof(x86_percpu, default_tss.rsp0) == PERCPU_KERNEL_SP_OFFSET);
static_assert(__offsetof(x86_percpu, interrupt_stacks.nmi) == PERCPU_INTERRUPT_STACKS_NMI_OFFSET);

static_assert(sizeof(ktl::declval<x86_percpu>().interrupt_stacks.nmi) == INTERRUPT_STACK_SIZE);

extern x86_percpu bp_percpu;

x86_percpu& x86_get_percpu(cpu_num_t);

x86_percpu* x86_get_percpu_for_test(cpu_num_t);

// This needs to be run very early in the boot process from
// ArchPostHandoffBootstrap() and as each CPU is brought up.
void x86_init_percpu(cpu_num_t cpu_num);

// used to set the bootstrap processor's apic_id once the APIC is initialized
extern "C" void x86_set_local_apic_id(uint32_t apic_id);

extern "C" {
uint32_t cpp_x86_percpu_get_apic_id(cpu_num_t cpu_num);
uint32_t cpp_x86_curr_percpu_get_apic_id();
}

int x86_apic_id_to_cpu_num(uint32_t apic_id);

// Allocate all of the necessary structures for all of the APs to run.
zx_status_t x86_allocate_ap_structures(uint32_t* apic_ids, uint8_t cpu_count);

template <typename T, size_t Offset>
[[gnu::always_inline]] inline T x86_read_percpu_field() {
  static_assert((Offset & (alignof(T) - 1)) == 0, "Bad offset alignment");
  if constexpr (sizeof(T) == sizeof(uint32_t)) {
    return reinterpret_cast<T>(x86_read_gs_offset32(Offset));
  } else {
    static_assert(sizeof(T) == sizeof(uint64_t));
    return reinterpret_cast<T>(x86_read_gs_offset64(Offset));
  }
}

template <typename T, size_t Offset>
[[gnu::always_inline]] inline void x86_write_percpu_field(T value) {
  static_assert((Offset & (alignof(T) - 1)) == 0, "Bad offset alignment");
  if constexpr (sizeof(T) == sizeof(uint32_t)) {
    x86_write_gs_offset32(Offset, reinterpret_cast<uint32_t>(value));
  } else {
    static_assert(sizeof(T) == sizeof(uint64_t));
    x86_write_gs_offset64(Offset, reinterpret_cast<uint64_t>(value));
  }
}

#define READ_PERCPU_FIELD(field) \
  (x86_read_percpu_field<decltype(x86_percpu::field), offsetof(x86_percpu, field)>())

#define WRITE_PERCPU_FIELD(field, value) \
  (x86_write_percpu_field<decltype(x86_percpu::field), offsetof(x86_percpu, field)>(value))

inline x86_percpu* x86_get_percpu() { return READ_PERCPU_FIELD(direct); }

// Return a pointer to the high-level percpu struct for the calling CPU.
inline percpu* arch_get_curr_percpu() { return READ_PERCPU_FIELD(high_level_percpu); }

inline cpu_num_t arch_curr_cpu_num() { return READ_PERCPU_FIELD(cpu_num); }

extern uint8_t x86_num_cpus;
inline uint arch_max_num_cpus() { return x86_num_cpus; }

void x86_ipi_halt_handler() __NO_RETURN;

// Called from assembly.
extern "C" void x86_secondary_entry(ktl::atomic<unsigned int>* aps_still_booting, Thread* thread);

void x86_force_halt_all_but_local_and_bsp();

// Setup the high-level percpu struct pointer for |cpu_num|.
void arch_setup_percpu(cpu_num_t cpu_num, percpu* percpu);

inline void arch_set_restricted_flag(bool restricted) {
  WRITE_PERCPU_FIELD(in_restricted_mode, restricted ? 1 : 0);
}

inline bool arch_get_restricted_flag() { return READ_PERCPU_FIELD(in_restricted_mode); }

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_X86_INCLUDE_ARCH_X86_MP_H_
