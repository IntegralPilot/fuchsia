// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2009 Corey Tabaka
// Copyright (c) 2015 Intel Corporation
// Copyright (c) 2016 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <arch.h>
#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/arch/x86/boot-cpuid.h>
#include <lib/backtrace/global_cpu_context_exchange.h>
#include <lib/console.h>
#include <platform.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/mp.h>
#include <arch/ops.h>
#include <arch/regs.h>
#include <arch/vm.h>
#include <arch/x86/apic.h>
#include <arch/x86/descriptor.h>
#include <arch/x86/feature.h>
#include <arch/x86/interrupts.h>
#include <arch/x86/mmu.h>
#include <arch/x86/mmu_mem_types.h>
#include <arch/x86/mp.h>
#include <hwreg/x86msr.h>
#include <kernel/cpu.h>
#include <kernel/mp.h>
#include <kernel/percpu.h>
#include <lk/init.h>
#include <lk/main.h>
#include <vm/vm.h>

#include "arch/x86.h"

#define LOCAL_TRACE 0

#ifndef __GCC_HAVE_SYNC_COMPARE_AND_SWAP_16
#error "missing -mcx16"
#endif

namespace {

int GetContext(int argc, const cmd_args* argv, uint32_t flags) {
  auto print_usage = [&]() {
    printf("usage:\n");
    printf("%s context <cpu_id> <timeout_ms>\n", argv[0].str);
  };

  if (argc < 4) {
    printf("not enough arguments\n");
    print_usage();
    return 1;
  }

  cpu_num_t target = static_cast<cpu_num_t>(argv[2].u);
  if (target >= percpu::processor_count()) {
    printf("invalid cpu_id: %u\n", target);
    return 1;
  }

  const uint64_t timeout_ms = argv[3].u;
  printf("requesting context of CPU-%u with timeout %lums\n", target, timeout_ms);

  CpuContext context;
  zx_status_t status;
  {
    InterruptDisableGuard irqd;
    status = g_cpu_context_exchange.RequestContext(target, ZX_MSEC(timeout_ms), context);
  }
  if (status != ZX_OK) {
    printf("error: %d\n", status);
    return 1;
  }
  context.backtrace.Print();
  PrintFrame(context.frame);

  return 0;
}

}  // namespace

void arch_early_init(void) {
  x86_mmu_early_init();

  // Mark the boot cpu as online here after global constructors have run
  mp_set_curr_cpu_online(true);
}

void arch_prevm_init(void) {
  x86_cpu_feature_init();
  // Perform some MMU initialization that is dependent on having done the previous feature init.
  x86_mmu_prevm_init();
}

void arch_init(void) {
  const struct x86_model_info* model = x86_get_model();
  printf("Processor Model Info: type %#x family %#x model %#x stepping %#x\n",
         model->processor_type, model->family, model->model, model->stepping);
  printf("\tdisplay_family %#x display_model %#x\n", model->display_family, model->display_model);

  x86_feature_debug();

  x86_mmu_init();

  gdt_setup();
  idt_setup_readonly();
}

void arch_late_init_percpu(void) {
  // Call per-CPU init function for the boot CPU.
  x86_cpu_feature_late_init_percpu();
}

void arch_prepare_uspace(const UserEntryState* state, iframe_t* out) {
  // The %fs.base (thread pointer) isn't in the iframe.  It's directly in the
  // CPU's register for the current thread.  It's always accessible via MSR,
  // and accessible directly with wrfsbase only on newer CPUs.
  write_msr(X86_MSR_IA32_FS_BASE, state->tp);

  *out = {
      .rdi = state->arg1,
      .rsi = state->arg2,
      .r15 = state->abi_reg,

      .ip = state->pc,
      .cs = USER_CODE_64_SELECTOR,

      // Default user space flags: IOPL 0; interrupts enabled.
      .flags = (0 << X86_FLAGS_IOPL_SHIFT) | X86_FLAGS_IF,

      .user_sp = state->sp,
      .user_ss = USER_DATA_SELECTOR,
  };
}

void arch_enter_uspace(const iframe_t* iframe) {
  DEBUG_ASSERT(arch_ints_disabled());

  LTRACEF("entry %#" PRIxPTR " user stack %#" PRIxPTR "\n", iframe->ip, iframe->user_sp);
  LTRACEF("kernel stack %#" PRIxPTR "\n", x86_get_percpu()->default_tss.rsp0);
#if __has_feature(safe_stack)
  LTRACEF("kernel unsafe stack %#" PRIxPTR "\n", Thread::Current::Get()->stack().unsafe_top());
#endif

  /* check that we are accessing userspace code */
  ASSERT(arch_is_valid_user_pc(iframe->ip));

  /* check that we're still pointed at the kernel gs */
  DEBUG_ASSERT(is_kernel_address(read_msr(X86_MSR_IA32_GS_BASE)));

  /* check that the kernel stack is set properly */
  DEBUG_ASSERT(is_kernel_address(x86_get_percpu()->default_tss.rsp0));

#if __has_feature(safe_stack)
  /* set the kernel unsafe stack back to the top as we enter user space */
  auto unsafe_top = Thread::Current::Get()->stack().unsafe_top();
  x86_uspace_entry(iframe, unsafe_top);
#else
  x86_uspace_entry(iframe);
#endif
  __UNREACHABLE;
}

[[noreturn, gnu::noinline]] static void finish_secondary_entry(
    ktl::atomic<unsigned int>* aps_still_booting, Thread* thread, uint cpu_num) {
  // Mark this cpu as online so MP code can try to deliver IPIs.
  // Mark here so any code waiting for the cpu to be started will see the cpu
  // online after the atomic below.
  mp_set_curr_cpu_online(true);

  // Signal that this CPU is initialized.  It is important that after this
  // operation, we do not touch any resources associated with bootstrap
  // besides our Thread and stack, since this is the checkpoint the
  // bootstrap process uses to identify completion.
  unsigned int old_val = aps_still_booting->fetch_and(~(1U << cpu_num));
  if (old_val == 0) {
    // If the value is already zero, then booting this CPU timed out.
    goto fail;
  }

  // Defer configuring memory settings until after the atomic_and above.
  // This ensures that we were in no-fill cache mode for the duration of early
  // AP init.
  DEBUG_ASSERT(x86_get_cr0() & X86_CR0_CD);
  x86_mmu_percpu_init();

  // Load the appropriate PAT/MTRRs.  This must happen after init_percpu, so
  // that this CPU is considered online.
  x86_pat_sync(1U << cpu_num);

  /* run early secondary cpu init routines up to the threading level */
  lk_init_level(LK_INIT_FLAG_SECONDARY_CPUS, LK_INIT_LEVEL_EARLIEST, LK_INIT_LEVEL_THREADING - 1);

  thread->SecondaryCpuInitEarly();

  lk_secondary_cpu_entry();

// lk_secondary_cpu_entry only returns on an error, halt the core in this
// case.
fail:
  arch_disable_ints();
  while (1) {
    x86_hlt();
  }
}

// This is called from assembly, before any other C code.
// The %gs.base is not set up yet, so we have to trust that
// this function is simple enough that the compiler won't
// want to generate stack-protector prologue/epilogue code,
// which would use %gs.
__NO_SAFESTACK __NO_RETURN void x86_secondary_entry(ktl::atomic<unsigned int>* aps_still_booting,
                                                    Thread* thread) {
  // Would prefer this to be in init_percpu, but there is a dependency on a
  // page mapping existing, and the BP calls that before the VM subsystem is
  // initialized.
  apic_local_init();

  uint32_t local_apic_id = apic_local_id();
  int cpu_num = x86_apic_id_to_cpu_num(local_apic_id);
  if (cpu_num < 0) {
    // If we could not find our CPU number, do not proceed further
    arch_disable_ints();
    while (1) {
      x86_hlt();
    }
  }

  DEBUG_ASSERT(cpu_num > 0);

  // Set %gs.base to our percpu struct.  This has to be done before
  // calling x86_init_percpu, which initializes most of that struct, so
  // that x86_init_percpu can use safe-stack and/or stack-protector code.
  x86_percpu& percpu = x86_get_percpu(cpu_num);
  write_msr(X86_MSR_IA32_GS_BASE, reinterpret_cast<uintptr_t>(&percpu));

  // Copy the stack-guard value from the boot CPU's perpcu.
  percpu.stack_guard = bp_percpu.stack_guard;

#if __has_feature(safe_stack)
  // Set up the initial unsafe stack pointer.
  DEBUG_ASSERT(IS_ROUNDED(thread->stack().unsafe_top(), 16));
  x86_write_gs_offset64(ZX_TLS_UNSAFE_SP_OFFSET, thread->stack().unsafe_top());
#endif

  x86_init_percpu((uint)cpu_num);

  // Now do the rest of the work, in a function that is free to
  // use %gs in its code.
  finish_secondary_entry(aps_still_booting, thread, cpu_num);
}

static int cmd_cpu(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
    printf("not enough arguments\n");
  usage:
    printf("usage:\n");
    printf("%s features\n", argv[0].str);
    printf("%s rdmsr <cpu_id> <msr_id>\n", argv[0].str);
    printf("%s wrmsr <cpu_id> <msr_id> <value>\n", argv[0].str);
    printf("%s lbr <subcommand>\n", argv[0].str);
    printf("%s context <cpu_id> <timeout_ms>\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  }

  if (!strcmp(argv[1].str, "features")) {
    x86_feature_debug();
  } else if (!strcmp(argv[1].str, "rdmsr")) {
    if (argc != 4) {
      goto usage;
    }

    uint64_t val = read_msr_on_cpu((uint)argv[2].u, (uint)argv[3].u);
    printf("CPU %lu RDMSR %lxh val %lxh\n", argv[2].u, argv[3].u, val);
  } else if (!strcmp(argv[1].str, "wrmsr")) {
    if (argc != 5) {
      goto usage;
    }

    printf("CPU %lu WRMSR %lxh val %lxh\n", argv[2].u, argv[3].u, argv[4].u);
    write_msr_on_cpu((uint)argv[2].u, (uint)argv[3].u, argv[4].u);
  } else if (!strcmp(argv[1].str, "context")) {
    return GetContext(argc, argv, flags);
  } else {
    printf("unknown command\n");
    goto usage;
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND("cpu", "cpu test commands", &cmd_cpu)
STATIC_COMMAND_END(cpu)
