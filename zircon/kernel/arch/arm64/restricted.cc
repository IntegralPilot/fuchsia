// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch.h>
#include <inttypes.h>
#include <stdlib.h>
#include <trace.h>

#include <arch/arm64/feature.h>
#include <arch/arm64/registers.h>
#include <arch/debugger.h>
#include <arch/regs.h>
#include <arch/vm.h>
#include <kernel/restricted_state.h>
#include <kernel/thread.h>

#define LOCAL_TRACE 0

extern "C" {
bool cpp_arm64_ints_disabled();
void cpp_arm64_get_tpidr_regs(uint64_t* tpidr_el0, uint64_t* tpidrro_el0);
void cpp_arm64_set_tpidr_regs(uint64_t tpidr_el0, uint64_t tpidrro_el0);
void cpp_arm64_enter_restricted_tpidr(uint64_t tpidr_el0, bool is_arm32);
uint64_t cpp_arm64_get_tpidr_el0();
[[noreturn]] void cpp_arm64_enter_uspace(const iframe_t* iframe);
zx_status_t cpp_arm64_get_general_regs(zx_thread_state_general_regs_t* regs);
zx_status_t cpp_arm64_set_general_regs(const zx_thread_state_general_regs_t* regs);

bool cpp_arm64_ints_disabled() { return arch_ints_disabled(); }

void cpp_arm64_get_tpidr_regs(uint64_t* tpidr_el0, uint64_t* tpidrro_el0) {
  *tpidr_el0 = __arm_rsr64("tpidr_el0");
  *tpidrro_el0 = __arm_rsr64("tpidrro_el0");
}

void cpp_arm64_set_tpidr_regs(uint64_t tpidr_el0, uint64_t tpidrro_el0) {
  __arm_wsr64("tpidr_el0", tpidr_el0);
  __arm_wsr64("tpidrro_el0", tpidrro_el0);
  Thread* thread = Thread::Current::Get();
  thread->arch().tpidr_el0 = tpidr_el0;
  thread->arch().tpidrro_el0 = tpidrro_el0;
}

void cpp_arm64_enter_restricted_tpidr(uint64_t tpidr_el0, bool is_arm32) {
  __arm_wsr64("tpidr_el0", tpidr_el0);
  if (is_arm32) {
    __arm_wsr64("tpidrro_el0", tpidr_el0);
  }
}

uint64_t cpp_arm64_get_tpidr_el0() { return __arm_rsr64("tpidr_el0"); }

[[noreturn]] void cpp_arm64_enter_uspace(const iframe_t* iframe) {
  arch_enter_uspace(iframe);
  __UNREACHABLE;
}

zx_status_t cpp_arm64_get_general_regs(zx_thread_state_general_regs_t* regs) {
  return arch_get_general_regs(Thread::Current::Get(), regs);
}

zx_status_t cpp_arm64_set_general_regs(const zx_thread_state_general_regs_t* regs) {
  return arch_set_general_regs(Thread::Current::Get(), regs);
}

}  // extern "C"
