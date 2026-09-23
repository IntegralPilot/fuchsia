// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/zircon-internal/macros.h>
#include <string.h>
#include <sys/types.h>
#include <zircon/errors.h>
#include <zircon/syscalls/debug.h>
#include <zircon/types.h>

#include <arch/arm64.h>
#include <arch/arm64/registers.h>
#include <arch/debugger.h>
#include <arch/regs.h>
#include <kernel/thread.h>

// SS (="Single Step") is bit 0 in MDSCR_EL1.
static constexpr uint64_t kMdscrSSMask = 1;

// Single Step for PSTATE, see ARMv8 Manual C5.2.18, enable Single step for Process
static constexpr uint64_t kSSMaskSPSR = (1 << 21);

zx_status_t arch_get_general_regs(Thread* thread, zx_thread_state_general_regs_t* out) {
  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_get_general_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // Punt if registers aren't available. E.g.,
  // TODO(https://fxbug.dev/42105394): Registers aren't available in synthetic exceptions.
  if (thread->arch().suspended_general_regs == nullptr) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  iframe_t* in = thread->arch().suspended_general_regs;
  DEBUG_ASSERT(in);

  static_assert(sizeof(in->r) == sizeof(out->r), "");
  memcpy(out->r, in->r, sizeof(in->r));
  out->lr = in->lr;
  out->sp = in->usp;
  out->pc = in->elr;
  // Enable a view into user visible registers as well as those
  // settable by restricted mode.
  out->cpsr = in->spsr & kArmUserRestrictedVisibleFlags;
  out->tpidr = thread->arch().tpidr_el0;

  return ZX_OK;
}

zx_status_t arch_set_general_regs(Thread* thread, const zx_thread_state_general_regs_t* in) {
  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_set_general_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // Punt if registers aren't available. E.g.,
  // TODO(https://fxbug.dev/42105394): Registers aren't available in synthetic exceptions.
  if (thread->arch().suspended_general_regs == nullptr) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  iframe_t* out = thread->arch().suspended_general_regs;
  DEBUG_ASSERT(out);

  static_assert(sizeof(out->r) == sizeof(in->r), "arch_set_general_regs");
  memcpy(out->r, in->r, sizeof(in->r));
  out->lr = in->lr;
  out->usp = in->sp;
  out->elr = in->pc;
  if (arch_get_restricted_flag()) {
    // Preserve all flags outside of NCZV (the directly user visible set) when
    // in restricted mode.  This allows continuity in 32-bit or 64-bit without
    // allowing direct access to mode changes.
    out->spsr = (out->spsr & ~kArmNczvFlags) | (in->cpsr & kArmNczvFlags);
  } else {
    // A normal mode thread should only allow user visible flags to be set.
    // However, this function may be called on a thread that has transitioned
    // from restricted mode, and as such, the mask below ensures that any saved
    // restricted-mode-only flags are not preserved.
    out->spsr = (out->spsr & ~kArmUserRestrictedVisibleFlags) | (in->cpsr & kArm64UserVisibleFlags);
  }
  thread->arch().tpidr_el0 = in->tpidr;

  return ZX_OK;
}

zx_status_t arch_get_single_step(Thread* thread, zx_thread_state_single_step_t* out) {
  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_get_single_step")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // Punt if registers aren't available. E.g.,
  // TODO(https://fxbug.dev/42105394): Registers aren't available in synthetic exceptions.
  if (thread->arch().suspended_general_regs == nullptr) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  iframe_t* regs = thread->arch().suspended_general_regs;

  const bool mdscr_ss_enable = !!(thread->arch().mdscr_el1 & kMdscrSSMask);
  const bool spsr_ss_enable = !!(regs->spsr & kSSMaskSPSR);

  *out = mdscr_ss_enable && spsr_ss_enable;
  return ZX_OK;
}

zx_status_t arch_set_single_step(Thread* thread, const zx_thread_state_single_step_t* in) {
  if (*in != 0 && *in != 1) {
    return ZX_ERR_INVALID_ARGS;
  }

  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_set_single_step")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // Punt if registers aren't available. E.g.,
  // TODO(https://fxbug.dev/42105394): Registers aren't available in synthetic exceptions.
  if (thread->arch().suspended_general_regs == nullptr) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  iframe_t* regs = thread->arch().suspended_general_regs;
  if (*in) {
    thread->arch().mdscr_el1 |= kMdscrSSMask;
    regs->spsr |= kSSMaskSPSR;
  } else {
    thread->arch().mdscr_el1 &= ~kMdscrSSMask;
    regs->spsr &= ~kSSMaskSPSR;
  }
  return ZX_OK;
}

zx_status_t arch_get_fp_regs(Thread* thread, zx_thread_state_fp_regs_t* out) {
  // There are no ARM fp regs.
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t arch_set_fp_regs(Thread* thread, const zx_thread_state_fp_regs_t* in) {
  // There are no ARM fp regs.
  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t arch_get_vector_regs(Thread* thread, zx_thread_state_vector_regs_t* out) {
  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_get_vector_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  const fpstate* in = &thread->arch().fpstate;
  out->fpcr = in->fpcr;
  out->fpsr = in->fpsr;
  for (int i = 0; i < 32; i++) {
    out->v[i].low = in->regs[i * 2];
    out->v[i].high = in->regs[i * 2 + 1];
  }

  return ZX_OK;
}

zx_status_t arch_set_vector_regs(Thread* thread, const zx_thread_state_vector_regs_t* in) {
  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_set_vector_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  fpstate* out = &thread->arch().fpstate;
  out->fpcr = in->fpcr;
  out->fpsr = in->fpsr;
  for (int i = 0; i < 32; i++) {
    out->regs[i * 2] = in->v[i].low;
    out->regs[i * 2 + 1] = in->v[i].high;
  }

  return ZX_OK;
}

zx_status_t arch_get_debug_regs(Thread* thread, zx_thread_state_debug_regs_t* out) {
  *out = {};
  out->hw_bps_count = arm64_hw_breakpoint_count();
  out->hw_wps_count = arm64_hw_watchpoint_count();

  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_get_debug_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // The kernel ensures that this state is being kept up to date, so we can safely copy the
  // information over.

  // HW breakpoints.
  for (size_t i = 0; i < out->hw_bps_count; i++) {
    out->hw_bps[i].dbgbcr = thread->arch().debug_state.hw_bps[i].dbgbcr;
    out->hw_bps[i].dbgbvr = thread->arch().debug_state.hw_bps[i].dbgbvr;
  }

  // Watchpoints.
  for (size_t i = 0; i < out->hw_wps_count; i++) {
    out->hw_wps[i].dbgwcr = thread->arch().debug_state.hw_wps[i].dbgwcr;
    out->hw_wps[i].dbgwvr = thread->arch().debug_state.hw_wps[i].dbgwvr;
  }

  out->esr = thread->arch().debug_state.esr;
  out->far = thread->arch().debug_state.far;

  return ZX_OK;
}

zx_status_t arch_set_debug_regs(Thread* thread, const zx_thread_state_debug_regs_t* in) {
  arm64_debug_state_t state = {};

  // We copy over the state from the input.
  uint64_t bp_count = arm64_hw_breakpoint_count();
  for (size_t i = 0; i < bp_count; i++) {
    state.hw_bps[i].dbgbcr = in->hw_bps[i].dbgbcr;
    state.hw_bps[i].dbgbvr = in->hw_bps[i].dbgbvr;
  }

  uint64_t wp_count = arm64_hw_watchpoint_count();
  for (size_t i = 0; i < wp_count; i++) {
    state.hw_wps[i].dbgwcr = in->hw_wps[i].dbgwcr;
    state.hw_wps[i].dbgwvr = in->hw_wps[i].dbgwvr;
  }

  uint32_t active_breakpoints = 0;
  uint32_t active_watchpoints = 0;
  if (!arm64_validate_debug_state(&state, &active_breakpoints, &active_watchpoints)) {
    return ZX_ERR_INVALID_ARGS;
  }

  SingleChainLockGuard thread_guard{IrqSaveOption, thread->get_lock(),
                                    CLT_TAG("arch_set_debug_regs")};

  DEBUG_ASSERT(thread->IsUserStateSavedLocked());

  // If the suspended registers are not there, we cannot save the MDSCR values for this thread,
  // meaning that the debug HW state will be cleared almost immediatelly.
  // This should always be there.
  // TODO(https://fxbug.dev/42105394): Registers aren't available in synthetic exceptions.
  if (!thread->arch().suspended_general_regs) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  bool hw_debug_needed = (active_breakpoints > 0) || (active_watchpoints > 0);

  arm64_set_debug_state_for_thread(thread, hw_debug_needed);
  state.esr = thread->arch().debug_state.esr;
  state.far = thread->arch().debug_state.far;

  thread->arch().track_debug_state = true;
  thread->arch().debug_state = state;

  return ZX_OK;
}

vaddr_t arch_get_instruction_pointer(GeneralRegsSource source, void* gregs) {
  DEBUG_ASSERT_MSG(source == GeneralRegsSource::Iframe, "invalid source %u\n",
                   static_cast<uint32_t>(source));
  return (reinterpret_cast<iframe_t*>(gregs)->elr);
}

zx_status_t arch_set_return_instruction_pointer(GeneralRegsSource source, void* gregs, vaddr_t ip) {
  DEBUG_ASSERT_MSG(source == GeneralRegsSource::Iframe, "invalid source %u\n",
                   static_cast<uint32_t>(source));
  static_cast<iframe_t*>(gregs)->elr = ip;
  return ZX_OK;
}

uint8_t arch_get_hw_breakpoint_count() { return arm64_hw_breakpoint_count(); }

uint8_t arch_get_hw_watchpoint_count() { return arm64_hw_watchpoint_count(); }
