// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/descriptor-regs.h>
#include <lib/arch/x86/interrupt.h>
#include <lib/arch/x86/msr.h>
#include <lib/arch/x86/standard-segments.h>
#include <lib/arch/x86/system.h>
#include <lib/fit/defer.h>
#include <stdint.h>
#include <zircon/tls.h>

#include <ktl/array.h>
#include <ktl/utility.h>
#include <phys/exception.h>
#include <phys/main.h>
#include <phys/stack.h>

#include <ktl/enforce.h>

namespace {

// Global GDT and TSS.
arch::X86StandardSegments gStandardSegments;

// IDT with entries for all the hardware interrupts.  Any software INT $n
// instructions with n >= 32 will produce a #GP fault instead.
constexpr size_t kIdtEntries = static_cast<size_t>(arch::X86Interrupt::kFirstUserDefined);

ktl::array<arch::SystemSegmentDesc64, kIdtEntries> gIdt;

// Hardware pushes this much (and sometimes the error code).
constexpr size_t kHardwareFrameSize = sizeof(arch::X86InterruptFrame);

// Each entry-point defined by MakeIdtEntry makes sure an error code has been
// pushed (by pushing zero when the hardware hasn't pushed) and then pushes
// the vector number.
struct IdtFrame {
  uint64_t vector;
  uint64_t error_code;
  arch::X86InterruptFrame hw;
};
constexpr size_t kIdtFrameSize = sizeof(IdtFrame);

// There's a separate entry-point generated for each interrupt vector.  That
// code comes from MakeIdtEntry(), below.  They all lead to this common C++
// function, defined below.
void InterruptCommon(PhysExceptionState& exception_state);

// The stack when InterruptCommon() is called holds the software exception
// frame for PhysException to inspect and modify, which is exactly the
// PhysExceptionState structure.  This has space for the registers and
// exception state that matter to 64-bit software, but doesn't hold all the
// same fields as the hardware arch::X86InterruptFrame (cs, ss).
constexpr size_t kSoftFrameSize = sizeof(PhysExceptionState);
static_assert(kSoftFrameSize % BOOT_STACK_ALIGN == 0);

// The plain registers are all first in this frame, so all those can be stored
// without overwriting the IdtFrame that overlaps the end of the new frame.
static_assert(kSoftFrameSize - offsetof(PhysExceptionState, regs.rip) >= kIdtFrameSize);

// Each IDT entry's entry-point code just normalizes the stack so it contains
// the hardware interrupt frame including error code, plus the vector number
// (see IdtFrame, above).  Then it jumps to a common assembly path at the label
// `interrupt.common`, which is defined by DefineInterruptCommonAsm(), below.
template <arch::X86Interrupt Vector>
arch::SystemSegmentDesc64 MakeIdtEntry() {
  // This asm is mostly to define the entry-point symbol interrupt.<Vector>
  // itself, which happens in an alternate section so it doesn't interfere with
  // the instruction sequence of MakeIdtEntry() itself. In the straight-line
  // code, the assembly just materializes the address of that entry-point.
  // Within that section, we use `.subsection` to order the fragments we emit,
  // not for correctness but just for optimal packing and expected ordering
  // when reading the disassembly.  The `interrupt.common` code goes first via
  // `.subsection 0` and is cache-line-aligned; then each `interrupt.<Vector>`
  // goes in `.subection <Vector> + 1`, and they all are only byte-aligned for
  // maximal packing into cache lines starting with the common path.
  uintptr_t entry_pc;
  __asm__(
      // For CFI purposes, the CFA is set to the SP before the hardware pushed
      // the arch::X86InterruptFrame, i.e. the limit of phys_exception_stack.
      // The SP is already kHardwareFrameSize below that (or one word more for
      // the error code) on entry, so the CFA rule reflects that.  The register
      // rules for the registers captured in arch::X86InterruptFrame apply
      // immediately; .cfi_signal_frame indicates the "caller" is actually an
      // interrupted PC and not the return address after a call site.  All
      // other registers still have the same value as the interrupted "caller".
      // If the error code wasn't pushed by the hardware, this pushes a zero
      // and so adjusts the CFA rule down by another word.  It then (always)
      // pushes the vector number, and adjusts the CFA rule down again for the
      // jump to the common code below.
      R"""(
        .pushsection .text.interrupt, "ax", %%progbits
        .subsection %c[vector] + 1
        .balign 1
        .type interrupt.%c[vector], %%function
        interrupt.%c[vector]:
          .cfi_startproc simple
          .cfi_signal_frame
          .cfi_def_cfa %%rsp, %c[iframe_size]
          .cfi_offset %%rip, %c[rip_cfa_offset]
          .cfi_offset %%rflags, %c[rflags_cfa_offset]
          .cfi_offset %%rsp, %c[rsp_cfa_offset]
.if 0 // TODO(https://fxbug.dev/42073531): clang encodes wrong
          .cfi_offset %%cs, %c[cs_cfa_offset]
          .cfi_offset %%ss, %c[ss_cfa_offset]
.endif
          .ifeq %c[has_error_code]
            pushq $0
          .endif
          .cfi_adjust_cfa_offset 8
          .cfi_same_value %%rax
          .cfi_same_value %%rbx
          .cfi_same_value %%rcx
          .cfi_same_value %%rdx
          .cfi_same_value %%rdi
          .cfi_same_value %%rsi
          .cfi_same_value %%rdi
          .cfi_same_value %%rax
          .cfi_same_value %%rbx
          .cfi_same_value %%rcx
          .cfi_same_value %%rdx
          .cfi_same_value %%rsi
          .cfi_same_value %%rdi
          .cfi_same_value %%rbp
          .cfi_same_value %%r8
          .cfi_same_value %%r9
          .cfi_same_value %%r10
          .cfi_same_value %%r11
          .cfi_same_value %%r12
          .cfi_same_value %%r13
          .cfi_same_value %%r14
          .cfi_same_value %%r15
          .cfi_same_value %%fs.base
          .cfi_same_value %%gs.base
          pushq %[vector]
          .cfi_adjust_cfa_offset 8
          jmp interrupt.common
          int3
          .cfi_endproc
        .size interrupt.%c[vector], . - interrupt.%c[vector]
        .popsection
        lea interrupt.%c[vector](%%rip), %[entry_pc]
        )"""
      : [entry_pc] "=r"(entry_pc)
      : [vector] "i"(Vector), [iframe_size] "i"(kHardwareFrameSize),
        [rip_cfa_offset] "i"(-offsetof(arch::X86InterruptFrame, rip)),
        [cs_cfa_offset] "i"(-offsetof(arch::X86InterruptFrame, cs)),
        [rflags_cfa_offset] "i"(-offsetof(arch::X86InterruptFrame, rflags)),
        [rsp_cfa_offset] "i"(-offsetof(arch::X86InterruptFrame, rsp)),
        [ss_cfa_offset] "i"(-offsetof(arch::X86InterruptFrame, ss)),
        [has_error_code] "i"(arch::X86InterruptHasErrorCode(Vector)));

  // All vectors use IST1 to reload the SP to phys_exception_stack's limit.
  return arch::X86StandardSegments::MakeInterruptGate(entry_pc, /*ist=*/1);
}

// This has no actual effect where it's called/inlined.  It just serves to
// define the `interrupt.common` local assembly entry-point symbol.  This is
// the common tail for all the individual IDT entries' entry points.  This
// turns the normalized and augmented hardware stack frame into a complete
// frame in the PhysExceptionState layout, and calls C++ InterruptCommon(),
// below.  It also handles restoring that (possibly modified) state if C++
// InterruptCommon() returns.
void DefineInterruptCommonAsm() {
  constexpr size_t kFrameSizeDifference = kSoftFrameSize - kIdtFrameSize;
  __asm__ volatile(
      // The incoming stack state is as set up by an interrupt.<Vector>
      // entry-point as defined above, SP points at an IdtFrame.  For CFI
      // purposes, this is still the same "frame" with the same CFA at the
      // limit of the exception stack, so the CFA rule starts with the
      // kIdtFrameSize offset from the SP.
      //
      // The state in the PhysExceptionState is divided into four blocks:
      //  1. normal registers (handle_regs)
      //  2. %fs.base & %gs.base (handle_fsgs)
      //  3. data already saved in the IdtFrame (handle_idt_regs)
      // Each block has a meta-macro defined below, that takes the name
      // of a per-register macro to expand for each register in the block.
      //
      // Initially the CFI rules for registers found in the IdtFrame point to
      // those where hardware and interrupt.<Vector> already saved them.  All
      // other registers still have the same value as the interrupted "caller".
      // The same_*_value macros produce the CFI directives for all that.
      //
      // Then first thing, the stack is adjusted down to make space for the
      // full PhysExceptionState frame, and the CFA rule adjusted accordingly.
      // This makes space for save_reg_value to store all the normal registers
      // (and update their CFI rules to find them in the frame), which then
      // leaves them free as scratch.
      //
      // Next, several scratch registers (chosen in handle_its_regs, below) are
      // needed to load values from the IdtFrame where it overlaps the end of
      // the new PhysExceptionState frame.  Those are saved in their proper
      // PhysExceptionState slots.
      //
      // The RDMSR instruction requires two scratch registers to fetch each of
      // %fs.base and %gs.base (one would be required for each of RDFSBASE and
      // RDGSBASE, but we don't rely on those being available).  Those are then
      // stored in their PhysExceptionState slots, almost completing the setup.
      //
      // To maintain C++ ABI invariants, %gs.base is then reset to the
      // boot_thread_pointer (label defined in start.S).  Finally, the
      // InterruptCommon() C++ function is called with the PhysExceptionState.
      //
      // If and when that returns, everything is reversed, reloading each
      // register from the frame (where C++ code may have changed values) and
      // restoring each CFI rule to same-value or to its IdtFrame slot.  The
      // %cs and %ss slots in the IdtFrame aren't stored anywhere in the
      // PhysExceptionState, but IRET needs them in arch::X86InterruptFrame.
      // So these are spilled to call-saved registers around calling C++ and
      // restored from there, rather than from the PhysExceptionState.
      //
      // Finally, the stack is moved back up to the base of the hardware frame
      // (popping the additional two words of the IdtFrame) and the CFA rule
      // adjusted accordingly before the IRET instruction restores state.
      R"""(
        .pushsection .text.interrupt, "ax", %%progbits
        .subsection 0
        .balign 64
        .type interrupt.common, %%function
        interrupt.common:
          .cfi_startproc simple
          .cfi_signal_frame
          .cfi_def_cfa %%rsp, %c[idt_frame_size]

          .macro handle_regs handle_reg
            \handle_reg %%rax, %c[rax_offset]
            \handle_reg %%rbx, %c[rbx_offset]
            \handle_reg %%rcx, %c[rcx_offset]
            \handle_reg %%rdx, %c[rdx_offset]
            \handle_reg %%rsi, %c[rsi_offset]
            \handle_reg %%rdi, %c[rdi_offset]
            \handle_reg %%rbp, %c[rbp_offset]
            \handle_reg %%r8, %c[r8_offset]
            \handle_reg %%r9, (%c[r8_offset] + (8 * (9 - 8)))
            \handle_reg %%r10, (%c[r8_offset] + (8 * (10 - 8)))
            \handle_reg %%r11, (%c[r8_offset] + (8 * (11 - 8)))
            \handle_reg %%r12, (%c[r8_offset] + (8 * (12 - 8)))
            \handle_reg %%r13, (%c[r8_offset] + (8 * (13 - 8)))
            \handle_reg %%r14, (%c[r8_offset] + (8 * (14 - 8)))
            \handle_reg %%r15, (%c[r8_offset] + (8 * (15 - 8)))
          .endm

          .macro handle_fsgs handle_fsgs_reg
            \handle_fsgs_reg %%fs.base, %c[fsbase_offset], %[msr_fsbase]
            \handle_fsgs_reg %%gs.base, %c[gsbase_offset], %[msr_gsbase]
          .endm

          .macro handle_idt_regs handle_idt_reg
            \handle_idt_reg %c[idt_rip_offset], %c[rip_offset], %%rax, %%rip
            \handle_idt_reg %c[idt_cs_offset], -1, %%rbx, %%cs, %%r12
            \handle_idt_reg %c[idt_rflags_offset], %c[rflags_offset], %%rcx, %%rflags
            \handle_idt_reg %c[idt_rsp_offset], %c[rsp_offset], %%rdx, %%rsp
            \handle_idt_reg %c[idt_ss_offset], -1, %%rsi, %%ss, %%r13
            \handle_idt_reg %c[idt_error_code_offset], %c[error_code_offset], %%rdi
            \handle_idt_reg %c[idt_vector_offset], %c[vector_offset], %%r8
          .endm

          .macro same_reg_value reg, offset, msr=
            .cfi_same_value \reg
          .endm
          handle_regs same_reg_value
          handle_fsgs same_reg_value

          .macro same_idt_value idt_offset, offset, scratch, reg=, spill=
            .ifnb \reg
              .ifge \offset
                .cfi_offset \reg, \idt_offset - %c[idt_frame_size]
              .endif
            .endif
          .endm
          handle_idt_regs same_idt_value

          sub %[frame_size_difference], %%rsp
          .cfi_def_cfa_offset %c[soft_frame_size]

          .macro save_reg_value reg, offset, valuereg
            mov \valuereg, \offset(%%rsp)
            .cfi_offset \reg, \offset - %c[soft_frame_size]
          .endm
          .macro save_reg reg, offset
            save_reg_value \reg, \offset, \reg
          .endm

          handle_regs save_reg

          .macro spill_idt_reg idt_offset, spill, reg
            mov (%c[frame_size_difference] + \idt_offset)(%%rsp), \spill
            .ifnb \reg
              .ifnc %%cs,\reg // TODO(https://fxbug.dev/42073531): clang encodes wrong
              .ifnc %%ss,\reg // TODO(https://fxbug.dev/42073531): clang encodes wrong
              .cfi_register \reg, \spill
              .endif
              .endif
            .endif
          .endm
          .macro save_load_idt_reg idt_offset, offset, scratch, reg=, spill=
            .ifge \offset
              spill_idt_reg \idt_offset, \scratch, \reg
            .else
              spill_idt_reg \idt_offset, \spill, \reg
            .endif
          .endm
          .macro save_idt_reg idt_offset, offset, scratch, reg=, spill=
            .ifge \offset
              mov \scratch, \offset(%%rsp)
              .ifnb \reg
                .cfi_offset \reg, \offset - %c[soft_frame_size]
              .endif
            .endif
          .endm
          handle_idt_regs save_load_idt_reg
          handle_idt_regs save_idt_reg

          .macro save_fsgs reg, offset, msr
            mov \msr, %%ecx
            rdmsr
            shl $32, %%rdx
            or %%rdx, %%rax
            mov %%rax, \offset(%%rsp)
            .cfi_offset \reg, \offset - %c[soft_frame_size]
          .endm
          handle_fsgs save_fsgs

          lea boot_thread_pointer(%%rip), %%rax
          mov %[msr_gsbase], %%ecx
          mov %%rax, %%rdx
          shr $32, %%rdx
          wrmsr

          mov %%rsp, %%rdi
          call %P[InterruptCommon]

          .macro restore_fsgs reg, offset, msr
            mov \offset(%%rsp), %%rax
            mov \msr, %%ecx
            mov %%rax, %%rdx
            shr $32, %%rdx
            wrmsr
            .cfi_same_value \reg
          .endm
          handle_fsgs restore_fsgs

          .macro restore_load_idt_reg idt_offset, offset, scratch, reg=, spill=
            .ifnb \reg
              .ifge \offset
                mov \offset(%%rsp), \scratch
              .endif
            .endif
          .endm
          .macro restore_idt_reg idt_offset, offset, scratch, reg=, spill=
            .ifnb \spill
              mov \spill, (%c[frame_size_difference] + \idt_offset)(%%rsp)
            .else
              .ifnb \reg
                mov \scratch, (%c[frame_size_difference] + \idt_offset)(%%rsp)
              .endif
            .endif
            same_idt_value \idt_offset, \offset, \scratch, \reg
          .endm
          handle_idt_regs restore_load_idt_reg
          handle_idt_regs restore_idt_reg

          .macro restore_reg reg, offset
            mov \offset(%%rsp), \reg
            .cfi_same_value \reg
          .endm
          handle_regs restore_reg

          add $%c[soft_frame_size] - %c[iret_frame_size], %%rsp
          .cfi_def_cfa_offset %c[iret_frame_size]
          iretq

          .purgem handle_regs
          .purgem handle_idt_regs
          .purgem handle_fsgs
          .purgem same_reg_value
          .purgem save_reg_value
          .purgem save_reg
          .purgem spill_idt_reg
          .purgem save_load_idt_reg
          .purgem save_idt_reg
          .purgem save_fsgs
          .purgem restore_fsgs
          .purgem restore_load_idt_reg
          .purgem restore_idt_reg

          .cfi_endproc
        .size interrupt.common, . - interrupt.common
        .popsection
      )"""
      :
      : [idt_frame_size] "i"(kIdtFrameSize),                // Incoming size.
        [soft_frame_size] "i"(kSoftFrameSize),              // Outgoing size.
        [frame_size_difference] "i"(kFrameSizeDifference),  // Difference.

        // The hardware frame used by the IRET instruction is not quite all of
        // the incoming IdtFrame, but most of it.
        [iret_frame_size] "i"(kHardwareFrameSize),

        // Incoming frame layout.
        [idt_rip_offset] "i"(offsetof(IdtFrame, hw.rip)),
        [idt_cs_offset] "i"(offsetof(IdtFrame, hw.cs)),
        [idt_rflags_offset] "i"(offsetof(IdtFrame, hw.rflags)),
        [idt_rsp_offset] "i"(offsetof(IdtFrame, hw.rsp)),
        [idt_ss_offset] "i"(offsetof(IdtFrame, hw.ss)),
        [idt_error_code_offset] "i"(offsetof(IdtFrame, error_code)),
        [idt_vector_offset] "i"(offsetof(IdtFrame, vector)),

        // Outgoing frame layout.
        [rax_offset] "i"(offsetof(PhysExceptionState, regs.rax)),
        [rbx_offset] "i"(offsetof(PhysExceptionState, regs.rbx)),
        [rcx_offset] "i"(offsetof(PhysExceptionState, regs.rcx)),
        [rdx_offset] "i"(offsetof(PhysExceptionState, regs.rdx)),
        [rsi_offset] "i"(offsetof(PhysExceptionState, regs.rsi)),
        [rdi_offset] "i"(offsetof(PhysExceptionState, regs.rdi)),
        [rsp_offset] "i"(offsetof(PhysExceptionState, regs.rsp)),
        [rbp_offset] "i"(offsetof(PhysExceptionState, regs.rbp)),
        [r8_offset] "i"(offsetof(PhysExceptionState, regs.r8)),
        [rip_offset] "i"(offsetof(PhysExceptionState, regs.rip)),
        [rflags_offset] "i"(offsetof(PhysExceptionState, regs.rflags)),
        [fsbase_offset] "i"(offsetof(PhysExceptionState, regs.fs_base)),
        [gsbase_offset] "i"(offsetof(PhysExceptionState, regs.gs_base)),
        [error_code_offset] "i"(offsetof(PhysExceptionState, exc.arch.u.x86_64.err_code)),
        [vector_offset] "i"(offsetof(PhysExceptionState, exc.arch.u.x86_64.vector)),

        [msr_fsbase] "i"(arch::X86Msr::IA32_FS_BASE),  // For RDMSR.
        [msr_gsbase] "i"(arch::X86Msr::IA32_GS_BASE),  // For RDMSR and WRMSR.

        [InterruptCommon] "i"(InterruptCommon));
}

// The assembly entry-point interrupt.common (above) calls this.  It's reset
// %gs.base to the boot_thread_pointer address, but not accessed that memory.
// It's filled in all of the regs fields and most of the exc fields.  The
// unsafe SP is still from the interrupted state, so this doesn't use it.
PHYS_SINGLETHREAD __NO_SAFESTACK void InterruptCommon(PhysExceptionState& exception_state) {
#if __has_feature(safe_stack)
  constexpr auto set_unsafe_sp = [](uint64_t unsafe_sp) {
    __asm__ volatile("mov %[unsafe_sp], %%gs:%c[offset]"
                     :
                     : [unsafe_sp] "r"(unsafe_sp), [offset] "i"(ZX_TLS_UNSAFE_SP_OFFSET));
  };

  // Save the interrupted unsafe stack pointer in the exception context, and
  // restore it (possibly modified) from there if we return.  Note this is
  // using the %gs.base value already reset to boot_thread_pointer by the
  // interrupt.common assembly code (above).  We assume that C++ code in the
  // PhysException call won't have moved %gs.base, so we can restore the unsafe
  // SP the same way.  When we ultimately return to the interrupt.common
  // assembly code (above), it will restore the %gs.base value stored in the
  // PhysExceptionState frame in case that's different.
  __asm__("mov %%gs:%c[offset], %[unsafe_sp]"
          : [unsafe_sp] "=r"(exception_state.unsafe_sp)
          : [offset] "i"(ZX_TLS_UNSAFE_SP_OFFSET));
  auto restore_unsafe_sp = fit::defer([set_unsafe_sp, &unsafe_sp = exception_state.unsafe_sp]() {
    // If the interrupted state was already on the exception stack, that
    // indicates an exception inside an exception handler.  The second
    // exception started over at the limit of the exception stack so any
    // interrupted state has already been clobbered and cannot be restored.
    if (phys_exception_unsafe_stack.IsOnStack(unsafe_sp)) {
      ZX_PANIC("PhysExceptionState attempting to resume unsafe SP %#" PRIx64
               " on exception unsafe stack",
               unsafe_sp);
    }
    set_unsafe_sp(unsafe_sp);
  });

  // Now reset it for PhysException and anything it calls to use.
  set_unsafe_sp(phys_exception_unsafe_stack.InitialSp());
#endif

  // The interrupt.common assembly code (above) has filled in all of regs and
  // most of the exc fields.  Just fill in the rest now.
  auto& exc_arch = exception_state.exc.arch.u.x86_64;
  exc_arch.cr2 = arch::X86Cr2::Read().address();
  exception_state.exc.synth_code = 0;
  exception_state.exc.synth_data = 0;

  const auto v = static_cast<arch::X86Interrupt>(exc_arch.vector);
  const char* name = arch::X86InterruptName(v);
  uint64_t result = PhysException(exc_arch.vector, name, exception_state);

  // If that returns at all, it must use this exact return value to indicate it
  // was on purpose and state should be restored.
  if (result != PHYS_EXCEPTION_RESUME) {
    ZX_PANIC("PhysException returned %#" PRIx64 " != expected %#" PRIx64, result,
             PHYS_EXCEPTION_RESUME);
  }

  // If the interrupted state was already on the exception stack, that
  // indicates an exception inside an exception handler.  The second exception
  // started over at the limit of the exception stack so any interrupted state
  // has already been clobbered and cannot be restored.
  if (phys_exception_stack.IsOnStack(exception_state.regs.rsp)) {
    ZX_PANIC("PhysException attempting to resume SP %#" PRIx64 " on exception stack",
             exception_state.regs.rsp);
  }

  // On return, the `interrupt.common` assembly code (above) restores state
  // from what's found in the PhysExceptionState frame still on the stack.
}

// This expands to instantiating all the individual IDT entry points with
// MakeIdtEntry<Vector> calls.
template <size_t... I>
void MakeIdtEntries(ktl::index_sequence<I...>) {
  static_assert(sizeof...(I) == kIdtEntries);
  gIdt = {MakeIdtEntry<static_cast<arch::X86Interrupt>(I)>()...};
}

void MakeIdtEntries() { MakeIdtEntries(ktl::make_index_sequence<kIdtEntries>()); }

void SetUpIdt() {
  // This doesn't do anything at runtime, it's just here to make sure the
  // assembly entry-point above is defined at compile time.
  DefineInterruptCommonAsm();

  // Each IDT entry is an interrupt gate using IST1 as its interrupt SP.  So
  // every vector always starts with SP at the limit of phys_exception_stack.
  gStandardSegments.tss().ist[0] = phys_exception_stack.InitialSp();

  // Initialize all the entries.  This call leads to instantiating every
  // MakeIdtEntry<Vector>(), which in turns emits each interrupt.<Vector>
  // assembly entry-point that the corresponding entry points to.
  MakeIdtEntries();

  // Finally, load the IDT pointer into the CPU.
  // Hereafter any exceptions should be handled.
  arch::LoadIdt(arch::GdtRegister64::Make(ktl::span(gIdt)));
}

}  // namespace

// There isn't even a definition for this type since there's nothing
// that needs to go there. But the pointer variable needs to exist.
ArchPhysInfo* gArchPhysInfo;

void ArchSetUpCpu() {
  gStandardSegments.Load();
  SetUpIdt();
}

void ArchSetUpZbi(EarlyBootZbi zbi) {}

uint64_t PhysExceptionResume(PhysExceptionState& state, uint64_t pc, uint64_t sp, uint64_t psr) {
  state.regs.rip = pc;
  state.regs.rsp = sp;
  state.regs.rflags = psr;
  return PHYS_EXCEPTION_RESUME;
}
