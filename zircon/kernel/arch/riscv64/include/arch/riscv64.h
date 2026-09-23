// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_H_

#define RISCV64_CSR_SMODE_BITS (1 << 8)

// These CSRs are only in user CSR space (still readable by all modes though)
#define RISCV64_CSR_CYCLE (0xc00)
#define RISCV64_CSR_TIME (0xc01)
#define RISCV64_CSR_INSRET (0xc02)
#define RISCV64_CSR_CYCLEH (0xc80)
#define RISCV64_CSR_TIMEH (0xc81)
#define RISCV64_CSR_INSRETH (0xc82)

#define RISCV64_CSR_SSTATUS (0x000 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SIE (0x004 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_STVEC (0x005 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SCOUNTEREN (0x006 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SENVCFG (0x00a | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SSCRATCH (0x040 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SEPC (0x041 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SCAUSE (0x042 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_STVAL (0x043 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_SIP (0x044 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_STIMECMP (0x04d | RISCV64_CSR_SMODE_BITS)  // extension Sstc
#define RISCV64_CSR_SATP (0x080 | RISCV64_CSR_SMODE_BITS)
#define RISCV64_CSR_VLENB (0xc22)

#define RISCV64_CSR_SSTATUS_SIE (1ul << 1)
#define RISCV64_CSR_SSTATUS_SPIE (1ul << 5)
#define RISCV64_CSR_SSTATUS_UBE (1ul << 6)
#define RISCV64_CSR_SSTATUS_SPP (1ul << 8)
#define RISCV64_CSR_SSTATUS_VS_SHIFT (9)
#define RISCV64_CSR_SSTATUS_VS_MASK (3ul << 9)
#define RISCV64_CSR_SSTATUS_VS_OFF (0ul)
#define RISCV64_CSR_SSTATUS_VS_INITIAL (1ul << 9)
#define RISCV64_CSR_SSTATUS_VS_CLEAN (2ul << 9)
#define RISCV64_CSR_SSTATUS_VS_DIRTY (3ul << 9)
#define RISCV64_CSR_SSTATUS_FS_SHIFT (13)
#define RISCV64_CSR_SSTATUS_FS_MASK (3ul << 13)
#define RISCV64_CSR_SSTATUS_FS_OFF (0ul)
#define RISCV64_CSR_SSTATUS_FS_INITIAL (1ul << 13)
#define RISCV64_CSR_SSTATUS_FS_CLEAN (2ul << 13)
#define RISCV64_CSR_SSTATUS_FS_DIRTY (3ul << 13)
#define RISCV64_CSR_SSTATUS_SUM (1ul << 18)
#define RISCV64_CSR_SSTATUS_MXR (1ul << 19)
#define RISCV64_CSR_SSTATUS_UXL_MASK (3ul << 32)
#define RISCV64_CSR_SSTATUS_UXL_32BIT (1ul << 32)
#define RISCV64_CSR_SSTATUS_UXL_64BIT (2ul << 32)
#define RISCV64_CSR_SSTATUS_UXL_128BIT (3ul << 32)
#define RISCV64_CSR_SSTATUS_SD (1ul << 63)

#define RISCV64_CSR_SIE_SSIE (1ul << 1)
#define RISCV64_CSR_SIE_STIE (1ul << 5)
#define RISCV64_CSR_SIE_SEIE (1ul << 9)

#define RISCV64_CSR_SIP_SSIP (1ul << 1)
#define RISCV64_CSR_SIP_STIP (1ul << 5)
#define RISCV64_CSR_SIP_SEIP (1ul << 9)

#define RISCV64_CSR_SCOUNTEREN_CY (1ul << 0)
#define RISCV64_CSR_SCOUNTEREN_TM (1ul << 1)
#define RISCV64_CSR_SCOUNTEREN_IR (1ul << 2)

#define RISCV64_CSR_SENVCFG_FIOM (1ul << 0)
#define RISCV64_CSR_SENVCFG_CBIE_MASK (3ul << 4)
#define RISCV64_CSR_SENVCFG_CBIE_ILLEGAL (0ul << 4)
#define RISCV64_CSR_SENVCFG_CBIE_FLUSH (1ul << 4)
#define RISCV64_CSR_SENVCFG_CBIE_INVAL (3ul << 4)
#define RISCV64_CSR_SENVCFG_CBCFE (1ul << 6)
#define RISCV64_CSR_SENVCFG_CBZE (1ul << 7)

#define RISCV64_CSR_VTYPE_VLMUL_SHIFT (0)
#define RISCV64_CSR_VTYPE_VLMUL_MASK (7ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_EIGHTH (5ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_QUARTER (6ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_HALF (7ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_ONE (0ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_TWO (1ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_FOUR (2ul << 0)
#define RISCV64_CSR_VTYPE_VLMUL_EIGHT (3ul << 0)
#define RISCV64_CSR_VTYPE_VSEW_SHIFT (3)
#define RISCV64_CSR_VTYPE_VSEW_MASK (7ul << 3)
#define RISCV64_CSR_VTYPE_VSEW_8 (0ul << 3)
#define RISCV64_CSR_VTYPE_VSEW_16 (1ul << 3)
#define RISCV64_CSR_VTYPE_VSEW_32 (2ul << 3)
#define RISCV64_CSR_VTYPE_VSEW_64 (3ul << 3)
#define RISCV64_CSR_VTYPE_VTA (1ul << 6)
#define RISCV64_CSR_VTYPE_VMA (1ul << 7)
#define RISCV64_CSR_VTYPE_VILL (1ul << 63)

// Interrupts, top bit set in cause register
#define RISCV64_INTERRUPT_SSWI 1  // software interrupt
#define RISCV64_INTERRUPT_STIM 5  // timer interrupt
#define RISCV64_INTERRUPT_SEXT 9  // external interrupt

// Exceptions
#define RISCV64_EXCEPTION_IADDR_MISALIGN 0
#define RISCV64_EXCEPTION_IACCESS_FAULT 1
#define RISCV64_EXCEPTION_ILLEGAL_INS 2
#define RISCV64_EXCEPTION_BREAKPOINT 3
#define RISCV64_EXCEPTION_LOAD_ADDR_MISALIGN 4
#define RISCV64_EXCEPTION_LOAD_ACCESS_FAULT 5
#define RISCV64_EXCEPTION_STORE_ADDR_MISALIGN 6
#define RISCV64_EXCEPTION_STORE_ACCESS_FAULT 7
#define RISCV64_EXCEPTION_ENV_CALL_U_MODE 8
#define RISCV64_EXCEPTION_ENV_CALL_S_MODE 9
#define RISCV64_EXCEPTION_ENV_CALL_M_MODE 11
#define RISCV64_EXCEPTION_INS_PAGE_FAULT 12
#define RISCV64_EXCEPTION_LOAD_PAGE_FAULT 13
#define RISCV64_EXCEPTION_STORE_PAGE_FAULT 15

// Byte offsets corresponding to the fields of riscv64_context_switch_frame.
#define REGOFF(x) ((x) * 8)
#define CONTEXT_SWITCH_FRAME_OFFSET_RA REGOFF(0)
#define CONTEXT_SWITCH_FRAME_OFFSET_TP REGOFF(1)
#define CONTEXT_SWITCH_FRAME_OFFSET_GP REGOFF(2)
#define CONTEXT_SWITCH_FRAME_OFFSET_S(n) REGOFF(3 + n)

#define SIZEOF_CONTEXT_SWITCH_FRAME REGOFF(14)

#ifndef __ASSEMBLER__

#include <assert.h>
#include <lib/arch/asm.h>
#include <stdbool.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <arch/regs.h>
#include <kernel/cpu.h>
#include <syscalls/syscalls.h>

#define __ASM_STR(x) #x

#define riscv64_csr_clear(csr, bits)                                            \
  ({                                                                            \
    ulong __val = bits;                                                         \
    __asm__ volatile("csrc   " __ASM_STR(csr) ", %0" ::"rK"(__val) : "memory"); \
  })

#define riscv64_csr_read_clear(csr, bits)                 \
  ({                                                      \
    ulong __val = bits;                                   \
    ulong __val_out;                                      \
    __asm__ volatile("csrrc   %0, " __ASM_STR(csr) ", %1" \
                     : "=r"(__val_out)                    \
                     : "rK"(__val)                        \
                     : "memory");                         \
    __val_out;                                            \
  })

#define riscv64_csr_set(csr, bits)                                              \
  ({                                                                            \
    ulong __val = bits;                                                         \
    __asm__ volatile("csrs   " __ASM_STR(csr) ", %0" ::"rK"(__val) : "memory"); \
  })

#define riscv64_csr_read(csr)                                               \
  ({                                                                        \
    ulong __val;                                                            \
    __asm__ volatile("csrr   %0, " __ASM_STR(csr) : "=r"(__val)::"memory"); \
    __val;                                                                  \
  })

#define riscv64_csr_write(csr, val)                                             \
  ({                                                                            \
    ulong __val = (ulong)val;                                                   \
    __asm__ volatile("csrw   " __ASM_STR(csr) ", %0" ::"rK"(__val) : "memory"); \
    __val;                                                                      \
  })

struct ArchPhysHandoff;

// Register state layout used by riscv64_context_switch().
struct alignas(16) riscv64_context_switch_frame {
  uint64_t ra;  // return address (x1)
  uint64_t tp;  // thread pointer
  uint64_t gp;  // shadow-call-stack pointer (x3)

  uint64_t s0;  // x8-x9
  uint64_t s1;

  uint64_t s2;  // x18-x26
  uint64_t s3;
  uint64_t s4;
  uint64_t s5;
  uint64_t s6;
  uint64_t s7;
  uint64_t s8;
  uint64_t s9;
  uint64_t s10;

  static constexpr auto kShadowCallStackPointer = &riscv64_context_switch_frame::gp;
};

static_assert(__offsetof(riscv64_context_switch_frame, ra) == CONTEXT_SWITCH_FRAME_OFFSET_RA);
static_assert(__offsetof(riscv64_context_switch_frame, tp) == CONTEXT_SWITCH_FRAME_OFFSET_TP);
static_assert(__offsetof(riscv64_context_switch_frame, s0) == CONTEXT_SWITCH_FRAME_OFFSET_S(0));
static_assert(sizeof(riscv64_context_switch_frame) == SIZEOF_CONTEXT_SWITCH_FRAME);
static_assert(sizeof(riscv64_context_switch_frame) % 16u == 0u);

// Defined in stvec.S: the entry point for all exceptions; stvec points to it.
extern "C" arch::AsmLabel Riscv64ExceptionEntry;

// The Riscv64ExceptionEntry code calls one of these three.  The sepc and
// sstatus values have already been stored in the iframe, but they are also
// handy in the argument registers already.
using Riscv64ExceptionFn = void(iframe_t* iframe, uint64_t pc, uint64_t status, int64_t cause);
extern "C" Riscv64ExceptionFn Riscv64UserException, Riscv64KernelException,
    Riscv64EmergencyException;

extern "C" void riscv64_context_switch(vaddr_t* old_sp, vaddr_t new_sp);
extern "C" syscall_result riscv64_syscall_dispatcher(iframe_t* frame);

extern "C" void riscv64_timer_exception();
extern "C" void riscv64_software_exception();

extern "C" void riscv64_init_percpu();

// Runs the boot cpu init routine with the boot hart id. This will do basic
// initialization of the cpu such as initializing the main control registers
// and loading the exception vector table and per cpu register.
extern "C" void riscv64_boot_cpu_init(const ArchPhysHandoff* arch_handoff);
inline void riscv64_boot_cpu_init(const ArchPhysHandoff& arch_handoff) {
  riscv64_boot_cpu_init(&arch_handoff);
}

#define __wfi() __asm__ volatile("wfi" ::: "memory")

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_H_
