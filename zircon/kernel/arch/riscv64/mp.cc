// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "arch/riscv64/mp.h"

#include <assert.h>
#include <lib/arch/riscv64/paging-traits.h>
#include <platform.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/mp.h>
#include <arch/mp_unplug_event.h>
#include <arch/ops.h>
#include <arch/riscv64.h>
#include <arch/riscv64/mmu.h>
#include <arch/riscv64/sbi.h>
#include <dev/interrupt.h>
#include <fbl/alloc_checker.h>
#include <hwreg/array.h>
#include <kernel/ffi.h>
#include <ktl/type_traits.h>
#include <ktl/unique_ptr.h>
#include <lk/init.h>
#include <lk/main.h>
#include <vm/handoff-end.h>
#include <vm/vm.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

// total number of detected cpus, accessed via arch_max_num_cpus()
uint riscv64_num_cpus = 1;

namespace {

using Paging = arch::Paging<arch::RiscvSv39PagingTraits>;

// PhysicalBootstrap provides the first kernel code to run on a secondary CPU.
// That code is entered from SBI with the MMU disabled and just two registers
// to bootstrap from.  a0 holds the hart ID, and a1 holds a "context" value.
// This object's physical address is used as that context value.
class PhysicalBootstrap {
 public:
  // This creates the PhysicalBootstrap for a secondary CPU to bootstrap into
  // running as the given Thread and CPU number.  The object resides on the new
  // thread's stack.  It's never explicitly destroyed, but it's trivially
  // destructible.  It's implicitly destroyed when the new CPU starts running:
  // that CPU consumes its values in the SbiEntry() assembly code before
  // clobbering the space as its own C++ stack when it enters VirtualEntry().
  static PhysicalBootstrap& Create(Thread& t, cpu_num_t cpu_num) {
    static_assert(ktl::is_trivially_destructible_v<PhysicalBootstrap>);
    uintptr_t top = t.stack().top();
    DEBUG_ASSERT(top % alignof(PhysicalBootstrap) == 0);
    void* storage = reinterpret_cast<PhysicalBootstrap*>(top) - 1;
    return *new (storage) PhysicalBootstrap{t, cpu_num};
  }

  // This returns the physical code address to give SBI as the entry point,
  // while giving it the _physical_ address of a PhysicalBootstrap object as
  // the "context" (a1) value.
  [[gnu::const]] static paddr_t SbiEntryPaddr() {
    // Translate the kernel code address to a physical address.
    // This part really could be done once and cached.
    return KernelPhysicalAddressOf(reinterpret_cast<uintptr_t>(SbiEntry));
  }

 private:
  explicit PhysicalBootstrap(Thread& t, cpu_num_t cpu_num)
      : sp_{t.stack().top()},
#if __has_feature(shadow_call_stack)
        gp_{t.stack().shadow_call_base()},
#endif
        tp_{reinterpret_cast<uintptr_t>(&t.arch().thread_pointer_location)},
        cpu_num_{cpu_num} {
    // This doesn't produce any code, but tells the compiler each member is an
    // input memory operand, so it knows all the values are going to be read.
    // This prevents the compiler from complaining or from eliding any of the
    // constructor initializer stores above.  The later atomic_thread_fence and
    // SBI's rules then guarantee the new CPU's loads in SbiEntry() get those
    // values via this object's `this` pointer passed as SBI's context pointer.
    __asm__ volatile(""
                     :  // GCC handles a single input like "m"(*this) of any
                        // size.  But Clang wants each field individually or
                        // else it complains and might think they are unused.
                        // Each operand should just be some N(a0) so it won't
                        // cost anything to set them up regardless.
                     : "m"(satp_), "m"(entry_),  //
                       "m"(ra_), "m"(sp_), "m"(gp_), "m"(tp_), "m"(cpu_num_));
  }

  // This is the first thing a secondary CPU runs with a virtual address PC.
  // It's never expected to return, but it is allowed to.
  static void VirtualEntry(uint32_t hart_id, cpu_num_t cpu_num);

  // This is not really a C++ function, it's only an assembly entry point.
  // Its physical address entry point is where the secondary CPU starts.
  //
  // SBI passes its hart ID in a0 and the "context" value in a1.  That
  // value is the physical address of the PhysicalBootstrap object.
  // While the MMU is still disabled, it loads all the essential register
  // values directly from that physical address, including the satp to
  // apply to enable the MMU and the virtual address to jump to.  The
  // effect is to call VirtualEntry with all the C++ ABI and argument
  // registers in already place, and return address VirtualEntryFailure.
  //
  // VirtualEntry takes the hart ID in a0 (where SBI left it), and the
  // cpu_num_t in a1.  The trampoline here loads all the compiler ABI
  // registers, using a2..a3 as scratch.  This sets ra as if by a normal
  // call.  It loads the satp CSR with the required fence.  It loads the
  // cpu_num_t into a1 last, as a1 was the physical address of the
  // PhysicalBootstrap object.  Finally, it jumps to the entry point.
  //
  // Note the percpu_ptr register (s11) is not yet set up.  Just in case,
  // it's reset to zero rather than leaving any incoming garbage there
  // before it's initialized inside VirtualEntry.
  [[noreturn, gnu::naked]]
  // The compiler will insert instrumentation even into a naked function!
  // This attribute ensures that it won't.
  [[gnu::no_profile_instrument_function]] static void SbiEntry(  //
      uint64_t hart_id,                                          // a0
      PhysicalBootstrap* p) {                                    // a1
    __asm__ volatile(
        R"""(
        .cfi_def_cfa zero, 0
        .cfi_return_column ra
        .cfi_undefined ra
        ld a2, %[offsetof_satp](a1)
        ld a3, %[offsetof_entry](a1)
        ld ra, %[offsetof_ra](a1)
        ld sp, %[offsetof_sp](a1)
        ld gp, %[offsetof_gp](a1)
        ld tp, %[offsetof_tp](a1)
        lw a1, %[offsetof_cpu_num](a1)
        mv s11, zero
        csrw satp, a2
        sfence.vma
        jr a3
        )"""
        :
        : [offsetof_satp] "i"(offsetof(PhysicalBootstrap, satp_)),
          [offsetof_entry] "i"(offsetof(PhysicalBootstrap, entry_)),
          [offsetof_ra] "i"(offsetof(PhysicalBootstrap, ra_)),
          [offsetof_sp] "i"(offsetof(PhysicalBootstrap, sp_)),
          [offsetof_gp] "i"(offsetof(PhysicalBootstrap, gp_)),
          [offsetof_tp] "i"(offsetof(PhysicalBootstrap, tp_)),
          [offsetof_cpu_num] "i"(offsetof(PhysicalBootstrap, cpu_num_)));
  }

  // This is used as the return address for calling VirtualEntry, so it runs if
  // that ever does return.  If a secondary CPU had some problem, it just spins
  // in place in hopes that another CPU can usefully diagnose what went wrong.
  static void VirtualEntryFailure() {
    [[unlikely]]
    while (true) {
      __asm__ volatile("wfi");
    }
  }

  // These values are all set in construction.  The ones initialized in their
  // declarations here are always the same for every CPU.  They are here just
  // to be read by the SbiEntryPaddr() bootstrap code.
  const uint64_t satp_ =
      // Initial identity-mapping for the low half allows continuing to the
      // next instruction after installing the satp, while full kernel virtual
      // space in the high half allows jumping to VirtualEntry.
      arch::RiscvSatp::Get()
          .FromValue(0)
          .set_mode(Paging::kMode)
          .set_asid(0)
          .set_root_address(riscv64_get_bootstrap_translation_table())
          .reg_value();
  const uint64_t entry_ = reinterpret_cast<uintptr_t>(VirtualEntry);
  const uint64_t ra_ = reinterpret_cast<uintptr_t>(VirtualEntryFailure);
  const uint64_t sp_;
  const uint64_t gp_ = 0;  // Always zero if !__has_feature(shadow_call_stack).
  const uint64_t tp_;
  const uint32_t cpu_num_;
};

// The vaddr on a kernel thread stack is in some arbitrary virtual mapping.
// But that mapping is fully populated and pinned and won't be changing now.
paddr_t StackPaddr(vaddr_t vaddr) {
  zx_paddr_t paddr;
  zx_status_t status = VmAspace::kernel_aspace()->arch_aspace().Query(vaddr, &paddr, nullptr);
  ZX_ASSERT_MSG(status == ZX_OK, "bad kernel stack vaddr %#" PRIxPTR ": error %d", vaddr, status);
  return paddr;
}

}  // anonymous namespace

extern "C" {
// C++ helpers called by Rust:
zx_status_t cpp_riscv64_start_cpu(cpu_num_t cpu_num, uint32_t hart_id) {
  return riscv64_start_cpu(cpu_num, hart_id);
}
}

// Called from the PhysicalBootstrap::SbiEntryPaddr() code.
void PhysicalBootstrap::VirtualEntry(uint32_t hart_id, cpu_num_t cpu_num) {
  riscv64_init_percpu();
  riscv64_mp_early_init_percpu(hart_id, cpu_num);
  riscv64_mmu_early_init_percpu();

  // The tp is already set, though not all of arch_thread is initialized yet.
  Thread::Current::Get()->SecondaryCpuInitEarly();

  // Run early secondary cpu init routines up to the threading level.
  lk_init_level(LK_INIT_FLAG_SECONDARY_CPUS, LK_INIT_LEVEL_EARLIEST, LK_INIT_LEVEL_THREADING - 1);

  arch_mp_init_percpu();

  dprintf(INFO, "RISCV: Secondary CPU %u running on hart ID %u\n", cpu_num, hart_id);

  // Should not return upon success.
  lk_secondary_cpu_entry();

  // The return address just enters VirtualEntryFailure(), defined above.
}

zx_status_t riscv64_start_cpu(cpu_num_t cpu_num, uint32_t hart_id) {
  LTRACEF("cpu %u, hart %u\n", cpu_num, hart_id);

  DEBUG_ASSERT(cpu_num > 0 && cpu_num < SMP_MAX_CPUS && hart_id != riscv64_boot_hart_id());

  // Allocate the thread and its stack.
  ktl::unique_ptr<Thread> thread;
  {
    char thread_name[ZX_MAX_NAME_LEN];
    snprintf(thread_name, sizeof(thread_name), "Start CPU %u hart %u", cpu_num, hart_id);
    fbl::AllocChecker ac;
    thread = fbl::make_unique_checked<Thread>(ac, thread_name);
    if (!ac.check()) {
      return ZX_ERR_NO_MEMORY;
    }
  }
  if (zx_status_t status = thread->stack().Init(); status != ZX_OK) {
    return status;
  }

  // Build the new CPU's bootstrap data structure, temporarily using the memory
  // that will become its new stack.  The trampoline code in SbiEntry() and
  // that bit of memory will be used via physical addresses to trampoline into
  // a call to VirtualEntry in a mostly-normal kernel C++ environment.
  PhysicalBootstrap& bootstrap = PhysicalBootstrap::Create(*thread, cpu_num);
  const paddr_t entry_paddr = PhysicalBootstrap::SbiEntryPaddr();
  const paddr_t context = StackPaddr(reinterpret_cast<uintptr_t>(&bootstrap));

  // Issue a memory barrier before starting the new CPU, to ensure previous
  // stores made on this CPU will be visible to the new CPU's entry point code.
  ktl::atomic_thread_fence(ktl::memory_order_release);

  // Tell SBI to start the secondary CPU.
  dprintf(INFO, "RISCV: Start CPU %u hart ID %u context {{{data:%#lx}}} entry {{{pc:%#lx}}}\n",
          cpu_num, hart_id, context, entry_paddr);
  zx_status_t status = power_cpu_on(hart_id, entry_paddr, context);
  if (status != ZX_OK) [[unlikely]] {
    KERNEL_OOPS("RISCV: Cannot start secondary CPU %u, hart ID %u: error %d\n", cpu_num, hart_id,
                status);
    return status;
  }

  // The secondary CPU is running and now owns its own Thread with stacks.
  ktl::ignore = thread.release();

  return ZX_OK;
}
