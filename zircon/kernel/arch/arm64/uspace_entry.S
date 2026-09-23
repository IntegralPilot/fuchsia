// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/arch_thread.h>
#include <arch/asm_macros.h>
#include <arch/regs.h>
#include <asm.h>
#include <zircon/compiler.h>

// void arm64_uspace_entry(iframe_t* iframe, vaddr_t kstack) __NO_RETURN;
// optionally vaddr_t scsp
FUNCTION(arm64_uspace_entry)
    mrs x16, tpidr_el1
#if __has_feature(shadow_call_stack)
    str x2, [x16, CURRENT_SCSP_OFFSET]
#endif
    ldr x16, [x16, CURRENT_MDSCR_OFFSET]
    msr mdscr_el1, x16

    mov sp, x1
    ldp x30, x16, [x0, ARM64_IFRAME_OFFSET_LR]  // LR is immediately followed by USP
    msr sp_el0, x16

    ldp x16, x17, [x0, ARM64_IFRAME_OFFSET_ELR]  // ELR is immediately followed by SPSR
    msr elr_el1, x16
    msr spsr_el1, x17

    // Move our iframe pointer into x28, and then proceed to restore x0-x29.
    // Leave x29 (the current frame pointer) the way it is for as long as we
    // can.  If we take a fault for any reason, we want our frame pointer to be
    // correct for when we attempt to produce a backtrace.
    mov x28, x0
    ldp x0, x1, [x28, ARM64_IFRAME_OFFSET_R + (0 * 8)]
    ldp x2, x3, [x28, ARM64_IFRAME_OFFSET_R + (2 * 8)]
    ldp x4, x5, [x28, ARM64_IFRAME_OFFSET_R + (4 * 8)]
    ldp x6, x7, [x28, ARM64_IFRAME_OFFSET_R + (6 * 8)]
    ldp x8, x9, [x28, ARM64_IFRAME_OFFSET_R + (8 * 8)]
    ldp x10, x11, [x28, ARM64_IFRAME_OFFSET_R + (10 * 8)]
    ldp x12, x13, [x28, ARM64_IFRAME_OFFSET_R + (12 * 8)]
    ldp x14, x15, [x28, ARM64_IFRAME_OFFSET_R + (14 * 8)]
    ldp x16, x17, [x28, ARM64_IFRAME_OFFSET_R + (16 * 8)]
    ldp x18, x19, [x28, ARM64_IFRAME_OFFSET_R + (18 * 8)]
    ldp x20, x21, [x28, ARM64_IFRAME_OFFSET_R + (20 * 8)]
    ldp x22, x23, [x28, ARM64_IFRAME_OFFSET_R + (22 * 8)]
    ldp x24, x25, [x28, ARM64_IFRAME_OFFSET_R + (24 * 8)]
    ldp x26, x27, [x28, ARM64_IFRAME_OFFSET_R + (26 * 8)]
    ldp x28, x29, [x28, ARM64_IFRAME_OFFSET_R + (28 * 8)]

    // Lazy loading of the FPU means we don't need to zero the simd registers
    eret
    SPECULATION_POSTFENCE
END_FUNCTION(arm64_uspace_entry)
