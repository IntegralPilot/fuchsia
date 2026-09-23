// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <bits.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/arch/arm64/system.h>
#include <lib/arch/intrin.h>
#include <lib/counters.h>
#include <lib/crashlog.h>
#include <platform.h>
#include <stdio.h>
#include <trace.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include <arch/arch_ops.h>
#include <arch/arm64.h>
#include <arch/arm64/uarch.h>
#include <arch/crashlog_regs.h>
#include <arch/exception.h>
#include <arch/regs.h>
#include <arch/thread.h>
#include <arch/user_copy.h>
#include <kernel/interrupt.h>
#include <kernel/thread.h>
#include <pretty/hexdump.h>
#include <vm/fault.h>
#include <vm/vm.h>

#define LOCAL_TRACE 0

namespace {

constexpr uint32_t DFSC_ALIGNMENT_FAULT = 0b100001;

using ESRExceptionClass = ::arch::ArmExceptionSyndromeRegister::ExceptionClass;

uint64_t kernel_addr_from_dfr(uint64_t dfr) {
  // Assert that the DFR is a valid kernel address by checking that the bit before the
  // ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT (which is before the ARM64_DFR_RUN_FAULT_HANDLER_BIT)
  // is a 1.
  DEBUG_ASSERT(BIT_SET(dfr, ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT - 1));
  return (dfr | (1ull << ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT) |
          (1ull << ARM64_DFR_RUN_FAULT_HANDLER_BIT));
}

// clang-format off
const char* dfsc_to_string(uint32_t dfsc) {
  switch (dfsc) {
    case 0b000000: return "Address Size Fault, Level 0";
    case 0b000001: return "Address Size Fault, Level 1";
    case 0b000010: return "Address Size Fault, Level 2";
    case 0b000011: return "Address Size Fault, Level 3";
    case 0b000100: return "Translation Fault, Level 0";
    case 0b000101: return "Translation Fault, Level 1";
    case 0b000110: return "Translation Fault, Level 2";
    case 0b000111: return "Translation Fault, Level 3";
    case 0b001001: return "Access Flag Fault, Level 1";
    case 0b001010: return "Access Flag Fault, Level 2";
    case 0b001011: return "Access Flag Fault, Level 3";
    case 0b001101: return "Permission Fault, Level 1";
    case 0b001110: return "Permission Fault, Level 2";
    case 0b001111: return "Permission Fault, Level 3";
    case 0b010000: return "Synchronous External Abort";
    case 0b010001: return "Synchronous Tag Check Fail";
    case 0b010100: return "Synchronous External Abort, Level 0";
    case 0b010101: return "Synchronous External Abort, Level 1";
    case 0b010110: return "Synchronous External Abort, Level 2";
    case 0b010111: return "Synchronous External Abort, Level 3";
    case 0b011000: return "Synchronous Parity or ECC Abort";
    case 0b011100: return "Synchronous Parity or ECC Abort, Level 0";
    case 0b011101: return "Synchronous Parity or ECC Abort, Level 1";
    case 0b011110: return "Synchronous Parity or ECC Abort, Level 2";
    case 0b011111: return "Synchronous Parity or ECC Abort, Level 3";
    case 0b100001: return "Alignment Fault";
    case 0b110000: return "TLB Conflict Abort";
    case 0b110100: return "Implementation Defined, Lockdown";
    case 0b110101: return "Implementation Defined, Unsupported exclusive or atomic";
    case 0b111101: return "Section Domain Fault";
    case 0b111110: return "Page Domain Fault";
    default: return "Unknown";
  }
}
// clang-format on

// Faulting Virtual Address for synchronous exceptions taken to EL1. Exceptions that
// set the FAR_EL1 are Instruction Aborts (EC 0x20 or 0x21), Data Aborts (EC 0x24 or
// 0x25), PC alignment faults (EC 0x22), and Watchpoints (EC 0x34 or 0x35).
// ESR_EL1.EC holds the EC value for the exception.
bool exception_sets_far(ESRExceptionClass ec) {
  switch (ec) {
    case ESRExceptionClass::kInstructionAbortLowerEl:
    case ESRExceptionClass::kInstructionAbortSameEl:
    case ESRExceptionClass::kDataAbortLowerEl:
    case ESRExceptionClass::kDataAbortSameEl:
    case ESRExceptionClass::kPcAlignment:
    case ESRExceptionClass::kWatchpointLowerEl:
    case ESRExceptionClass::kWatchpointSameEl:
      return true;
    default:
      return false;
  }
}

KCOUNTER(exceptions_brkpt, "exceptions.breakpoint")
KCOUNTER(exceptions_hw_brkpt, "exceptions.hw_breakpoint")
KCOUNTER(exceptions_hw_wp, "exceptions.hw_watchpoint")
KCOUNTER(exceptions_fpu, "exceptions.fpu")
KCOUNTER(exceptions_page, "exceptions.page_fault")
KCOUNTER(exceptions_irq, "exceptions.irq")
KCOUNTER(exceptions_unhandled, "exceptions.unhandled")
KCOUNTER(exceptions_user, "exceptions.user")
KCOUNTER(exceptions_unknown, "exceptions.unknown")
KCOUNTER(exceptions_access, "exceptions.access_fault")
KCOUNTER(exceptions_serror, "exceptions.serror")
KCOUNTER(exceptions_mops, "exceptions.mops")

zx_status_t try_dispatch_user_data_fault_exception(zx_excp_type_t type, iframe_t* iframe,
                                                   uint32_t esr, uint64_t far,
                                                   uint32_t error_code) {
  arch_exception_context_t context = {};
  DEBUG_ASSERT(iframe != nullptr);
  context.frame = iframe;
  context.esr = esr;
  context.far = far;
  context.user_synth_code = error_code;
  context.user_synth_data = 0;

  arch_enable_ints();
  zx_status_t status = dispatch_user_exception(type, &context);
  arch_disable_ints();
  return status;
}

// Must be called with interrupts disabled from exception entry.
zx_status_t try_dispatch_user_exception(zx_excp_type_t type, iframe_t* iframe, uint32_t esr) {
  auto esr_reg = arch::ArmExceptionSyndromeRegister::Get().FromValue(esr);
  static_assert(sizeof(esr_reg) <= sizeof(uint64_t) * 4);
  uint64_t maybe_far = exception_sets_far(esr_reg.ec()) ? __arm_rsr64("far_el1") : 0;
  return try_dispatch_user_data_fault_exception(type, iframe, esr, maybe_far, 0);
}

// Prints exception details and then panics.
[[noreturn, gnu::format(printf, 4, 5)]]
void exception_die(iframe_t* iframe, uint32_t esr, uint64_t far, const char* format, ...) {
  platform_panic_start();

  va_list args;
  va_start(args, format);
  vprintf(format, args);
  va_end(args);

  uint32_t ec = BITS_SHIFT(esr, 31, 26);
  uint32_t il = BIT(esr, 25);
  uint32_t iss = BITS(esr, 24, 0);

  /* fatal exception, die here */
  printf("ESR %#x: ec %#x, il %#x, iss %#x\n", esr, ec, il, iss);
  printf("FAR %#" PRIx64 "\n", far);
  PrintFrame(*iframe);
  g_crashlog.regs.iframe = iframe;
  g_crashlog.regs.esr = esr;
  g_crashlog.regs.far = far;
  g_crashlog.regs.tpidr_el0 = __arm_rsr64("tpidr_el0");
  g_crashlog.regs.tpidr_el1 = __arm_rsr64("tpidr_el1");
  g_crashlog.regs.tpidrro_el0 = __arm_rsr64("tpidrro_el0");

  platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
}

void arm64_unknown_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  /* this is for a lot of reasons, but most of them are undefined instructions */
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "unknown exception in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }
  try_dispatch_user_exception(ZX_EXCP_UNDEFINED_INSTRUCTION, iframe, esr);
}

void arm64_mops_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  // This means the PC and the PSTATE don't agree.  It's not an undefined
  // instruction but it's an illegal instruction.
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "FEAT_MOPS exception in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }
  try_dispatch_user_exception(ZX_EXCP_UNDEFINED_INSTRUCTION, iframe, esr);
}

void arm64_brk_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"), "BRK in kernel: PC at %#" PRIx64 "\n",
                  iframe->elr);
  }
  // Spectre V2: If we took a BRK exception from EL0, but the ELR address is not a user address,
  // invalidate the branch predictor. User code may be attempting to mistrain indirect branch
  // prediction structures.
  if (unlikely(!is_user_accessible(iframe->elr)) &&
      READ_PERCPU_FIELD(should_invalidate_bp_on_el0_exception)) {
    arm64_uarch_do_spectre_v2_mitigation();
  }
  try_dispatch_user_exception(ZX_EXCP_SW_BREAKPOINT, iframe, esr);
}

void arm64_pc_alignment_fault_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "PC alignment fault in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }

  try_dispatch_user_exception(ZX_EXCP_UNALIGNED_ACCESS, iframe, esr);
}

void arm64_hw_breakpoint_exception_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "HW breakpoint in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }

  // We don't need to save the debug state because it doesn't change by an exception. The only
  // way to change the debug state is through the thread write syscall.

  // NOTE: ARM64 Doesn't provide a good way to comunicate exception status (without exposing ESR
  //       to userspace). This means a debugger will have to compare the registers with the PC
  //       on the exceptions to find out which breakpoint triggered the exception.
  try_dispatch_user_exception(ZX_EXCP_HW_BREAKPOINT, iframe, esr);
}

void arm64_watchpoint_exception_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  // Arm64 uses the Fault Address Register to determine which watchpoint triggered the exception.
  uint64_t far = __arm_rsr64("far_el1");

  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, far, "Watchpoint in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }

  // We don't need to save the debug state because it doesn't change by an exception. The only
  // way to change the debug state is through the thread write syscall.

  try_dispatch_user_data_fault_exception(ZX_EXCP_HW_BREAKPOINT, iframe, esr, far, 0);
}

void arm64_step_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* trapped inside the kernel, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "software step in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }
  // TODO(https://fxbug.dev/42108005): Is it worth separating this into two separate exceptions?
  try_dispatch_user_exception(ZX_EXCP_HW_BREAKPOINT, iframe, esr);
}

void arm64_fpu_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
    /* we trapped a floating point instruction inside our own EL, this is bad */
    exception_die(iframe, esr, __arm_rsr64("far_el1"),
                  "invalid fpu use in kernel: PC at %#" PRIx64 "\n", iframe->elr);
  }
  exception_die(iframe, esr, __arm_rsr64("far_el1"), "unhandled fpu exception\n");
}

void arm64_instruction_abort_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  uint64_t far = __arm_rsr64("far_el1");
  uint32_t ec = BITS_SHIFT(esr, 31, 26);
  uint32_t iss = BITS(esr, 24, 0);
  uint32_t dfsc = BITS(iss, 5, 0);
  bool is_user = !BIT(ec, 0);
  // 0b0010XX is access faults.
  bool is_access = (dfsc & 0b111100) == 0b001000;
  // 0b0001XX is translation faults.
  bool is_translation = (dfsc & 0b111100) == 0b000100;

  if (unlikely(!is_user)) {
    // Any instruction page fault in kernel mode is a bug.
    exception_die(iframe, esr, far, "instruction abort in kernel mode\n");
  }

  // Strip any tag that might be present on the FAR.
  far = arch_detag_ptr(far);

  // Spectre V2: If we took an instruction abort in EL0 but the faulting address is not a user
  // address, invalidate the branch predictor. The $PC may have been updated before the abort is
  // delivered, user code may be attempting to mistrain indirect branch prediction structures.
  if (unlikely(is_user && !is_user_accessible(far)) &&
      READ_PERCPU_FIELD(should_invalidate_bp_on_el0_exception)) {
    arm64_uarch_do_spectre_v2_mitigation();
  }

  uint pf_flags = VMM_PF_FLAG_INSTRUCTION;
  pf_flags |= is_user ? VMM_PF_FLAG_USER : 0;
  pf_flags |= is_translation ? VMM_PF_FLAG_NOT_PRESENT : 0;
  pf_flags |= is_access ? VMM_PF_FLAG_ACCESS : 0;

  LTRACEF("instruction abort: PC at %#" PRIx64 ", is_user %d, FAR %" PRIx64 ", esr %#x, iss %#x\n",
          iframe->elr, is_user, far, esr, iss);

  DEBUG_ASSERT(arch_num_spinlocks_held() == 0);
  arch_enable_ints();
  zx_status_t err;
  // Check for accessed fault and update the counters accordingly.
  if (is_access) {
    DEBUG_ASSERT((pf_flags & VMM_PF_FLAG_ACCESS) != 0);
    kcounter_add(exceptions_access, 1);
  } else {
    kcounter_add(exceptions_page, 1);
  }
  err = vmm_page_fault_handler(far, pf_flags);
  arch_disable_ints();
  if (err == ZX_OK) {
    return;
  }

  // If this is from user space, let the user exception handler
  // get a shot at it.
  if (is_user) {
    kcounter_add(exceptions_user, 1);
    if (try_dispatch_user_data_fault_exception(ZX_EXCP_FATAL_PAGE_FAULT, iframe, esr, far,
                                               static_cast<uint32_t>(err)) == ZX_OK) {
      return;
    }
  }

  exception_die(iframe, esr, far,
                "instruction abort: PC at %#" PRIx64 ", is_user %d, FAR %" PRIx64 "\n", iframe->elr,
                is_user, far);
}

void arm64_data_abort_handler(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  /* read the FAR register */
  uint64_t far = __arm_rsr64("far_el1");
  uint32_t ec = BITS_SHIFT(esr, 31, 26);
  uint32_t iss = BITS(esr, 24, 0);
  uint32_t dfsc = BITS(iss, 5, 0);
  bool is_user = !BIT(ec, 0);
  // 0b0010XX is access faults.
  bool is_access = (dfsc & 0b111100) == 0b001000;
  // 0b0001XX is translation faults.
  bool is_translation = (dfsc & 0b111100) == 0b000100;
  bool WnR = BIT(iss, 6);  // Write not Read
  bool CM = BIT(iss, 8);   // cache maintenance op

  uint pf_flags = 0;
  // if it was marked Write but the cache maintenance bit was set, treat it as read
  pf_flags |= (WnR && !CM) ? VMM_PF_FLAG_WRITE : 0;
  pf_flags |= is_user ? VMM_PF_FLAG_USER : 0;
  pf_flags |= is_translation ? VMM_PF_FLAG_NOT_PRESENT : 0;
  pf_flags |= is_access ? VMM_PF_FLAG_ACCESS : 0;

  LTRACEF("data fault: PC at %#" PRIx64 ", is_user %d, FAR %#" PRIx64 ", esr %#x, iss %#x\n",
          iframe->elr, is_user, far, esr, iss);

  uint64_t dfr = Thread::Current::Get()->arch().data_fault_resume;
  if (unlikely(!is_user) && unlikely(!dfr)) {
    // Any page fault in kernel mode that's not during user-copy is a bug.
    exception_die(iframe, esr, far, "data abort in kernel mode\n");
  }

  // Check if we want to capture this fault.
  bool capture_fault = false;
  if (unlikely(dfr)) {
    if (unlikely(!BIT_SET(dfr, ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT))) {
      // This function does not support capturing only access faults. In other words, if the caller
      // wants to capture access faults, they must also capture page faults, so we assert that
      // here.
      DEBUG_ASSERT(!BIT_SET(dfr, ARM64_DFR_RUN_FAULT_HANDLER_BIT));
      capture_fault = true;
    } else if (unlikely(!BIT_SET(dfr, ARM64_DFR_RUN_FAULT_HANDLER_BIT))) {
      // If the RUN_FAULT_HANDLER_BIT is not set, then we only want to capture this fault if it is
      // _not_ an access fault.
      capture_fault = !is_access;
    }
  }

  // Only invoke the page fault handler for access, translation, and permission faults. Any other
  // kind of fault cannot be resolved by the handler.
  // 0b0010XX is access faults.
  // 0b0001XX is translation faults.
  // 0b0011XX is permission faults.
  const bool invoke_page_fault_handler = (dfsc & 0b001100) != 0 && (dfsc & 0b110000) == 0;

  if (capture_fault) {
    iframe->elr = kernel_addr_from_dfr(dfr);
    // Need to inform the user_copy routine whether this is a fault that can be handled by the page
    // fault handler or not, which is done by encoding a different status code.
    if (likely(invoke_page_fault_handler)) {
      // The user_copy routine expects the status and the flags combined into the low and high parts
      // of x0 respectively, and since status values are negative we need to construct this
      // carefully to control the sign extensions.
      iframe->r[0] = static_cast<uint64_t>(static_cast<uint32_t>(ZX_ERR_INVALID_ARGS)) |
                     (static_cast<uint64_t>(pf_flags) << 32);
      // TODO(https://fxbug.dev/42175395): x1 is relayed back to user_copy where it will be stored
      // in page fault info. Currently, the only users of this page fault info is
      // VmAspace::SoftFault, but the kernel page fault handler shouldn't accept/work with tags. To
      // avoid architecture-specific tags reaching the VM layer at all, we can strip it here so it
      // never reaches user_copy page fault results.
      iframe->r[1] = arch_detag_ptr(far);
    } else {
      iframe->r[0] = ZX_ERR_BAD_STATE;
    }
    return;
  }

  zx_status_t err = ZX_OK;
  if (likely(invoke_page_fault_handler)) {
    if (is_access) {
      DEBUG_ASSERT((pf_flags & VMM_PF_FLAG_ACCESS) != 0);
      kcounter_add(exceptions_access, 1);
    } else {
      kcounter_add(exceptions_page, 1);
    }
    DEBUG_ASSERT(arch_num_spinlocks_held() == 0);
    arch_enable_ints();
    err = vmm_page_fault_handler(arch_detag_ptr(far), pf_flags);
    arch_disable_ints();
    if (err == ZX_OK) {
      return;
    }
  }

  // Check if the current thread was expecting a data fault and
  // we should return to its handler.
  if (dfr && is_user_accessible(far)) {
    // Having the ARM64_DFR_RUN_FAULT_HANDLER_BIT and ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT bits
    // set should have already resulted in a valid sign extended canonical address. Double check the
    // bit before, which should be a one.
    DEBUG_ASSERT(BIT_SET(dfr, ARM64_DFR_RUN_ACCESS_FAULT_HANDLER_BIT - 1));
    iframe->elr = dfr;
    // Set the return status for the user copy routine.
    iframe->r[0] = ZX_ERR_INVALID_ARGS;
    return;
  }

  // If this is from user space, let the user exception handler
  // get a shot at it.
  if (is_user) {
    kcounter_add(exceptions_user, 1);
    zx_excp_type_t excp_type = ZX_EXCP_FATAL_PAGE_FAULT;
    if (unlikely(dfsc == DFSC_ALIGNMENT_FAULT)) {
      excp_type = ZX_EXCP_UNALIGNED_ACCESS;
    }
    if (try_dispatch_user_data_fault_exception(excp_type, iframe, esr, far,
                                               static_cast<uint32_t>(err)) == ZX_OK) {
      return;
    }
  }

  // Print the data fault and stop the kernel.
  exception_die(iframe, esr, far,
                "data fault: PC at %#" PRIx64 ", FAR %#" PRIx64
                "\n"
                "ISS %#x (WnR %d CM %d)\n"
                "DFSC %#x (%s)\n",
                iframe->elr, far, iss, WnR, CM, dfsc, dfsc_to_string(dfsc));
}

}  // anonymous namespace

/* called from assembly */
extern "C" void arm64_sync_exception(iframe_t* iframe, uint exception_flags, uint32_t esr) {
  auto esr_reg = arch::ArmExceptionSyndromeRegister::Get().FromValue(esr);

  switch (esr_reg.ec()) {
    case ESRExceptionClass::kUnknown:
      kcounter_add(exceptions_unknown, 1);
      arm64_unknown_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kFp:
      kcounter_add(exceptions_fpu, 1);
      arm64_fpu_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kSvc32:
    case ESRExceptionClass::kSvc64:
      exception_die(iframe, esr, __arm_rsr64("far_el1"),
                    "syscalls should be handled in assembly\n");
      break;
    case ESRExceptionClass::kInstructionAbortLowerEl:
    case ESRExceptionClass::kInstructionAbortSameEl:
      CPU_STATS_INC(page_faults);
      arm64_instruction_abort_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kDataAbortLowerEl:
    case ESRExceptionClass::kDataAbortSameEl:
      CPU_STATS_INC(page_faults);
      arm64_data_abort_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kBreakpointLowerEl:
    case ESRExceptionClass::kBreakpointSameEl:
      kcounter_add(exceptions_hw_brkpt, 1);
      arm64_hw_breakpoint_exception_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kStepLowerEl:
    case ESRExceptionClass::kStepSameEl:
      arm64_step_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kWatchpointLowerEl:
    case ESRExceptionClass::kWatchpointSameEl:
      kcounter_add(exceptions_hw_wp, 1);
      arm64_watchpoint_exception_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kBkpt:
    case ESRExceptionClass::kBrk:
      kcounter_add(exceptions_brkpt, 1);
      arm64_brk_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kPcAlignment:
      arm64_pc_alignment_fault_handler(iframe, exception_flags, esr);
      break;
    case ESRExceptionClass::kMops:
      kcounter_add(exceptions_mops, 1);
      arm64_mops_handler(iframe, exception_flags, esr);
      break;
    default: {
      /* TODO: properly decode more of these */
      if (unlikely((exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) == 0)) {
        /* trapped inside the kernel, this is bad */
        exception_die(iframe, esr, __arm_rsr64("far_el1"),
                      "unhandled exception in kernel: PC at %#" PRIx64 "\n", iframe->elr);
      }
      /* let the user exception handler get a shot at it */
      kcounter_add(exceptions_unhandled, 1);
      if (try_dispatch_user_exception(ZX_EXCP_GENERAL, iframe, esr) == ZX_OK) {
        break;
      }
      exception_die(iframe, esr, __arm_rsr64("far_el1"), "unhandled synchronous exception\n");
    }
  }

  /* if we came from user space, check to see if we have any signals to handle */
  if (exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL) {
    /* in the case of receiving a kill signal, this function may not return,
     * but the scheduler would have been invoked so it's fine.
     */
    arch_iframe_process_pending_signals(iframe);
  }
}

/* called from assembly */
extern "C" void arm64_irq(iframe_t* iframe, uint exception_flags);
extern "C" void arm64_irq(iframe_t* iframe, uint exception_flags) {
  LTRACEF("iframe %p, flags %#x\n", iframe, exception_flags);
  bool is_user = exception_flags & ARM64_EXCEPTION_FLAG_LOWER_EL;

  // Spectre V2: If we took an interrupt while in EL0 but $PC was not a user address, invalidate
  // the branch predictor. User code may be attempting to mistrain an indirect branch predictor.
  if (unlikely(is_user && !is_user_accessible(iframe->elr)) &&
      READ_PERCPU_FIELD(should_invalidate_bp_on_el0_exception)) {
    arm64_uarch_do_spectre_v2_mitigation();
  }

  int_handler_saved_state_t state;
  int_handler_start(&state);

  kcounter_add(exceptions_irq, 1);
  platform_irq(iframe);

  bool do_preempt = int_handler_finish(&state);

  /* preempt the thread if the interrupt has signaled it */
  if (do_preempt) {
    Thread::Current::Preempt();
  }

  /* if we came from user space, check to see if we have any signals to handle */
  if (unlikely(is_user)) {
    arch_iframe_process_pending_signals(iframe);
  }
}

/* called from assembly */
extern "C" void arm64_serror_exception(iframe_t* iframe, uint exception_flags);
extern "C" void arm64_serror_exception(iframe_t* iframe, uint exception_flags) {
  // SError is largely implementation defined and may or may not be fatal. For now, just count the
  // occurrences and add a tracer to help analyze possible causes.
  ktrace::Scope trace = KTRACE_CPU_BEGIN_SCOPE("kernel:irq", "irq", ("irq #", "SError"));
  exceptions_serror.Add(1);
}

/* called from assembly */
extern "C" void arm64_invalid_exception(iframe_t* iframe, unsigned int which);
extern "C" void arm64_invalid_exception(iframe_t* iframe, unsigned int which) {
  platform_panic_start();

  printf("invalid exception, which %#x\n", which);
  PrintFrame(*iframe);

  platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
}

/* called from assembly */
extern "C" void arm64_unrestricted_arm32_exception(iframe_t* iframe, unsigned int which,
                                                   uint32_t esr);
extern "C" void arm64_unrestricted_arm32_exception(iframe_t* iframe, unsigned int which,
                                                   uint32_t esr) {
  platform_panic_start();

  // See include/arch/arm64.h to decode the which value
  printf("unrestricted exception from arm32, esr %#x which %#x\n", esr, which);
  PrintFrame(*iframe);

  platform_halt(HALT_ACTION_HALT, ZirconCrashReason::Panic);
}

/* called from assembly */
extern "C" void arch_iframe_process_pending_signals(iframe_t* iframe) {
  DEBUG_ASSERT(iframe != nullptr);
  Thread::Current::ProcessPendingSignals(GeneralRegsSource::Iframe, iframe);
}

void arch_dump_exception_context(const arch_exception_context_t* context) {
  // Print the common fields first.
  dump_common_exception_context(context);

  // If we don't have a frame, there's nothing more we can print.
  if (context->frame == nullptr) {
    printf("no frame to dump\n");
    return;
  }

  auto esr = arch::ArmExceptionSyndromeRegister::Get().FromValue(context->esr);
  ESRExceptionClass ec = esr.ec();
  uint32_t iss = static_cast<uint32_t>(esr.iss());

  switch (ec) {
    case ESRExceptionClass::kInstructionAbortLowerEl:
    case ESRExceptionClass::kInstructionAbortSameEl:
      printf("instruction abort: PC at %#" PRIx64 ", address %#" PRIx64 " IFSC %#x %s\n",
             context->frame->elr, context->far, BITS(context->esr, 5, 0),
             BIT(static_cast<uint32_t>(ec), 0) ? "" : "user ");

      break;
    case ESRExceptionClass::kDataAbortLowerEl:
    case ESRExceptionClass::kDataAbortSameEl:
      printf("data abort: PC at %#" PRIx64 ", address %#" PRIx64 " %s%s\n", context->frame->elr,
             context->far, BIT(static_cast<uint32_t>(ec), 0) ? "" : "user ",
             BIT(iss, 6) ? "write" : "read");
      break;
    default:
      break;
  }

  PrintFrame(*context->frame);
  printf("tpidr_el0 %#35" PRIx64 "         tpidrro_el0 %#25" PRIx64 "\n",  //
         __arm_rsr64("tpidr_el0"), __arm_rsr64("tpidrro_el0"));

  // try to dump the user stack
  if (is_user_accessible(context->frame->usp)) {
    uint8_t buf[256];
    if (arch_copy_from_user(buf, reinterpret_cast<void*>(context->frame->usp), sizeof(buf)) ==
        ZX_OK) {
      printf("bottom of user stack at %#lx:\n", static_cast<vaddr_t>(context->frame->usp));
      hexdump_ex(buf, sizeof(buf), context->frame->usp);
    }
  }
}

void arch_fill_in_exception_context(const arch_exception_context_t* arch_context,
                                    zx_exception_report_t* report) {
  zx_exception_context_t* zx_context = &report->context;

  zx_context->synth_code = arch_context->user_synth_code;
  zx_context->synth_data = arch_context->user_synth_data;
  zx_context->arch.u.arm_64.esr = arch_context->esr;
  zx_context->arch.u.arm_64.far = arch_context->far;
}

zx_status_t arch_dispatch_user_policy_exception(uint32_t policy_exception_code,
                                                uint32_t policy_exception_data) {
  arch_exception_context_t context = {};
  context.user_synth_code = policy_exception_code;
  context.user_synth_data = policy_exception_data;
  return dispatch_user_exception(ZX_EXCP_POLICY_ERROR, &context);
}

bool arch_install_exception_context(Thread* thread, const arch_exception_context_t* context) {
  if (!context->frame) {
    // TODO(https://fxbug.dev/42105394): Must be a synthetic exception as they don't (yet) provide
    // the registers.
    return false;
  }

  arch_set_suspended_general_regs(thread, GeneralRegsSource::Iframe, context->frame);
  thread->arch().debug_state.esr = context->esr;
  thread->arch().debug_state.far = context->far;
  return true;
}

void arch_remove_exception_context(Thread* thread) { arch_reset_suspended_general_regs(thread); }
