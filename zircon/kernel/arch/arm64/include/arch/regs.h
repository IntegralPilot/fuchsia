// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_REGS_H_
#define ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_REGS_H_

#define ARM64_IFRAME_OFFSET_R (0 * 8)
#define ARM64_IFRAME_OFFSET_ELR (30 * 8)
#define ARM64_IFRAME_OFFSET_SPSR (31 * 8)
#define ARM64_IFRAME_OFFSET_LR (32 * 8)
#define ARM64_IFRAME_OFFSET_USP (33 * 8)
#define ARM64_IFRAME_SIZE ((30 + 4) * 8)

#ifndef __ASSEMBLER__

#include <stdint.h>
#include <zircon/compiler.h>

// Registers saved on entering the kernel via architectural exception.
// See the notes below for more details.
//
struct iframe_t {
  uint64_t r[30];  // x0-x29
  // **NOTE:** It is important that ELR immediately follows r[29] in memory.  See below.
  uint64_t elr;    // ELR_EL1
  uint64_t spsr;   // SPSR_EL1
  uint64_t lr;     // x30 (arm64 lr)
  uint64_t usp;    // either SP_EL0 if from lower EL otherwise the SP before the iframe was pushed
};

static_assert(sizeof(iframe_t) % 16u == 0u);
static_assert(sizeof(iframe_t) == ARM64_IFRAME_SIZE);

// The first 30 entries here are a direct mapping to r0-r29.  Typically, they
// would be followed by r30 (the link register). That said, the layout of the
// structure is technically arbitrary.  We just need to save all of the
// registers so we can properly restore them later on.
//
// In this case, we have re-arranged them just slightly so we can implement a
// small trick in our synchronous EL1 -> EL1 exception handlers.  Instead of
// placing the LR where r30 would normally be, we place the ELR there instead,
// followed by the SPSR, and then the saved value of r30 (the LR).
static_assert(__offsetof(iframe_t, r) == ARM64_IFRAME_OFFSET_R);
static_assert(__offsetof(iframe_t, r[29]) + sizeof(uint64_t) == __offsetof(iframe_t, elr));

// During an exception handler, after saving our registers, but before
// transferring control to the top level C exception handler, we can point r29
// (the frame pointer) at the FP/ELR (eg, r[29]/ELR) pair in our iframe.  Then
// we proceed as normal.
//
// If the exception turns out to be fatal, and we attempt to produce a backtrace
// during our panic handlers, it will look (to the backtrace code) like we have
// and extra frame on the stack, only the frame is embedded in our iframe
// structure instead of being a typical frame.  The FP in this frame points to
// the last valid frame before the exception happened.  IOW - the frame which
// points to the function which called the function which took the exception.
// By following this with the ELR, the backtrace code will see the address at
// which the exception was taken as the place that the virtual "exception
// handler frame" was called from.
//
// The cost of this (in addition to the structure re-ordering) is just a single
// instruction which loads x29 with the value of the stack pointer plus a
// constant offset.
//
// During unwind, the proper value of x29 will be reloaded using the r[29]
// member of the iframe, automatically destroying our "virtual frame" in the
// process.
static_assert(__offsetof(iframe_t, elr) == ARM64_IFRAME_OFFSET_ELR);
static_assert(__offsetof(iframe_t, spsr) == ARM64_IFRAME_OFFSET_SPSR);
static_assert(__offsetof(iframe_t, elr) + sizeof(uint64_t) == __offsetof(iframe_t, spsr));

// Additionally, the registers in the iframe are logically laid out in specific
// pairs, and *must* either remain this way, or a lot of lower level ASM code
// will need to be updated.  The low level ASM code uses load/store pair
// instructions when restoring/saving register state from/to the iframe. The
// first 30 registers (r0-29) are implicitly paired together by nature of the
// C-array.  After that, the following layout rules need to be enforced:
//
// 1) [r[0], r[29]] are all paired.  IOW, r[N+1] must always follow r[N] for all
//    even N in the range [0, 2, 4, ... 28].
// 2) ELR/SPSR are a pair; SPSR must always immediately follow ELR.
// 3) LR/USP are a pair; USP must always immediately follow LR.
// 4) ELR must always follow r[29].  This makes the "exception frame" trick
//    work.  See above.
static_assert(__offsetof(iframe_t, lr) == ARM64_IFRAME_OFFSET_LR);
static_assert(__offsetof(iframe_t, usp) == ARM64_IFRAME_OFFSET_USP);
static_assert(__offsetof(iframe_t, lr) + sizeof(uint64_t) == __offsetof(iframe_t, usp));

// Registers saved on entering the kernel via syscall.
using syscall_regs_t = iframe_t;

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_ARM64_INCLUDE_ARCH_REGS_H_
