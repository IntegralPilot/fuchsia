// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <inttypes.h>
#include <stdlib.h>
#include <trace.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>

#include <arch/debugger.h>
#include <arch/regs.h>
#include <arch/vm.h>
#include <arch/x86.h>
#include <arch/x86/descriptor.h>
#include <arch/x86/feature.h>
#include <kernel/restricted.h>
#include <vm/vm_address_region.h>

#define LOCAL_TRACE 0

namespace {

// helper routines to read/write user fs and gs base registers using the optimal
// mechanism. must be called with interrupts disabled around the swapgs sequence.
[[gnu::target("fsgsbase")]]
void get_fsgsbase(uint64_t* fsbase, uint64_t* gsbase) {
  DEBUG_ASSERT(arch_ints_disabled());

  // read the fs/gs base out of the MSRs
  if (likely(x86_feature_test(X86_FEATURE_FSGSBASE))) {
    *fsbase = _readfsbase_u64();
    // the user and kernel base have been swapped, use swapgs to temporarily
    // gain access to the gs register.
    __asm__ __volatile__("swapgs\n");
    uint64_t temp = _readgsbase_u64();
    __asm__ __volatile__("swapgs\n");
    *gsbase = temp;
  } else {
    *fsbase = read_msr(X86_MSR_IA32_FS_BASE);
    *gsbase = read_msr(X86_MSR_IA32_KERNEL_GS_BASE);
  }
}

[[gnu::target("fsgsbase")]]
void set_fsgsbase(uint64_t fsbase, uint64_t gsbase) {
  DEBUG_ASSERT(arch_ints_disabled());

  DEBUG_ASSERT(x86_is_vaddr_canonical(fsbase));
  DEBUG_ASSERT(x86_is_vaddr_canonical(gsbase));
  if (likely(x86_feature_test(X86_FEATURE_FSGSBASE))) {
    _writefsbase_u64(fsbase);
    // the user and kernel base have been swapped, use swapgs to temporarily
    // gain access to the gs register.
    __asm__ __volatile__("swapgs\n");
    _writegsbase_u64(gsbase);
    __asm__ __volatile__("swapgs\n");
  } else {
    write_msr(X86_MSR_IA32_FS_BASE, fsbase);
    write_msr(X86_MSR_IA32_KERNEL_GS_BASE, gsbase);
  }
}

}  // namespace

extern "C" {
void cpp_x86_get_fsgsbase_ints_disabled(uint64_t* fsbase, uint64_t* gsbase);
void cpp_x86_get_fsgsbase_ints_enabled(uint64_t* fsbase, uint64_t* gsbase);
void cpp_x86_set_fsgsbase(uint64_t fsbase, uint64_t gsbase);
[[noreturn]] void cpp_x86_enter_uspace(const iframe_t* iframe);
zx_status_t cpp_x86_get_general_regs(zx_thread_state_general_regs_t* regs);
zx_status_t cpp_x86_set_general_regs(const zx_thread_state_general_regs_t* regs);

void cpp_x86_get_fsgsbase_ints_disabled(uint64_t* fsbase, uint64_t* gsbase) {
  DEBUG_ASSERT(arch_ints_disabled());
  get_fsgsbase(fsbase, gsbase);
}

void cpp_x86_get_fsgsbase_ints_enabled(uint64_t* fsbase, uint64_t* gsbase) {
  DEBUG_ASSERT(!arch_ints_disabled());
  arch_disable_ints();
  get_fsgsbase(fsbase, gsbase);
  arch_enable_ints();
}

void cpp_x86_set_fsgsbase(uint64_t fsbase, uint64_t gsbase) {
  DEBUG_ASSERT(arch_ints_disabled());
  set_fsgsbase(fsbase, gsbase);
}

[[noreturn]] void cpp_x86_enter_uspace(const iframe_t* iframe) {
  arch_enter_uspace(iframe);
  __UNREACHABLE;
}

zx_status_t cpp_x86_get_general_regs(zx_thread_state_general_regs_t* regs) {
  return arch_get_general_regs(Thread::Current().Get(), regs);
}

zx_status_t cpp_x86_set_general_regs(const zx_thread_state_general_regs_t* regs) {
  return arch_set_general_regs(Thread::Current().Get(), regs);
}

}  // extern "C"
