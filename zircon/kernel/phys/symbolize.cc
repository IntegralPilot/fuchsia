// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/symbolize.h"

#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/elfldltl/diagnostics.h>
#include <stdarg.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <ktl/iterator.h>
#include <ktl/string_view.h>
#include <ktl/utility.h>
#include <ktl/variant.h>
#include <phys/elf-image.h>
#include <phys/main.h>
#include <phys/stack.h>
#include <pretty/hexdump.h>

// The zx_*_t types used in the exception stuff aren't defined for 32-bit.
// There is no exception handling implementation for 32-bit.
#ifndef __i386__
#include <phys/exception.h>
#endif

#include <ktl/enforce.h>

namespace {

using LoadInfo = ElfImage::LoadInfo;
using size_type = ElfImage::Elf::size_type;

auto PanicDiagnostics(ktl::string_view name) {
  return elfldltl::Diagnostics{
      elfldltl::PrintfDiagnosticsReport(__zx_panic, name, ": "),
      elfldltl::DiagnosticsPanicFlags(),
  };
}

void PruneSegment(LoadInfo::ZeroFillSegment& segment, size_type memsz) {
  segment = LoadInfo::ZeroFillSegment{segment.vaddr(), memsz};
}

void PruneSegment(LoadInfo::DataWithZeroFillSegment& segment, size_type memsz) {
  ZX_DEBUG_ASSERT(memsz >= segment.filesz());
  segment = LoadInfo::DataWithZeroFillSegment{
      segment.offset(),
      segment.vaddr(),
      memsz,
      segment.filesz(),
  };
}

void PruneSegment(auto& segment, size_type memsz) {
  // The final segment being pruned should only ever be either
  // DataWithZeroFillSegment or pure ZeroFillSegment.
  ZX_PANIC("impossible segment type for Symbolize::PruneFromMainModule!");
}

}  // namespace

Symbolize* gSymbolize = nullptr;

void Symbolize::ReplaceModulesStorage(ModuleList modules) {
  ModuleList old = ktl::exchange(modules_, ktl::move(modules));
  modules_.clear();
  for (const ElfImage* module : old) {
    AddModule(module);
  }
}

void Symbolize::AddModule(const ElfImage* module) {
  auto diag = PanicDiagnostics(name_);
  [[maybe_unused]] bool ok = modules_.push_back(diag, "too many modules loaded", module);
  ZX_DEBUG_ASSERT(ok);
}

const char* ProgramName() {
  if (gSymbolize) {
    return gSymbolize->name();
  }
  return "early-init";
}

elfldltl::ElfNote Symbolize::build_id() const {
  ZX_DEBUG_ASSERT(main_module_);
  ZX_DEBUG_ASSERT(main_module_->build_id());
  return main_module_->build_id().value();
}

void Symbolize::Printf(const char* fmt, ...) {
  va_list args;
  va_start(args, fmt);
  vfprintf(output_, fmt, args);
  va_end(args);
}

void Symbolize::ContextAlways(FILE* log) {
  decltype(writer_) log_writer{{log}};
  auto& writer = log ? log_writer : writer_;
  writer.Prefix(name_).Reset().Newline();
  for (size_t i = 0; i < modules_.size(); ++i) {
    modules_[i]->SymbolizerContext(writer, static_cast<unsigned int>(i), name_);
  }
}

void Symbolize::Context() {
  if (!context_done_) {
    context_done_ = true;
    ContextAlways();
  }
}

void Symbolize::ModuleContext(const ElfImage& loaded, unsigned int module_id, bool physical) {
  if (context_done_) {
    loaded.SymbolizerContext(writer_, module_id, name_, physical);
  }
}

void Symbolize::OnLoad(const ElfImage& loaded) {
  ModuleContext(loaded, static_cast<unsigned int>(modules_.size()));
  AddModule(&loaded);
}

const ElfImage* Symbolize::module_for_vaddr_range(uintptr_t vaddr, size_t len) const {
  auto contains_vaddr_range = [vaddr, len](const ElfImage* module) {
    return module->contains_vaddr_range(vaddr, len);
  };
  auto list = modules();
  auto it = ktl::ranges::find_if(list, contains_vaddr_range);
  if (it == list.end()) [[unlikely]] {
    return nullptr;
  }
  return *it;
}

void Symbolize::OnHandoff(ElfImage& next) {
  // The module will usually be the among the latest added, so search in
  // reverse.
  auto it = ktl::find(modules().rbegin(), modules().rend(), &next);
  ZX_ASSERT_MSG(it != modules().rend(), "Module %.*s has not been loaded",
                static_cast<int>(next.name().size()), next.name().data());
  next.OnHandoff();
  main_module_ = &next;
}

void Symbolize::LogHandoff(ktl::string_view name, uintptr_t entry_pc) {
  writer_.Prefix(name_).Literal({"Hand off to ", name, " at "}).Code(entry_pc).Newline();
}

void Symbolize::BackTraceFrame(unsigned int n, uintptr_t pc, bool interrupt) {
  // Just print the line in markup format.  Context() was called earlier.
  writer_.Prefix(name_);
  interrupt ? writer_.ExactPcFrame(n, pc) : writer_.ReturnAddressFrame(n, pc);
  writer_.Newline();
}

void Symbolize::DumpFile(ktl::string_view announce, size_t size_bytes, ktl::string_view sink_name,
                         ktl::string_view vmo_name, ktl::string_view vmo_name_suffix) {
  Context();
  writer_.Prefix(name_).Prefix(announce).Dumpfile(sink_name, vmo_name, vmo_name_suffix);
  Printf(" %zu bytes\n", size_bytes);
}

void Symbolize::PrintBacktraces(const Symbolize::FramePointerBacktrace& frame_pointers,
                                const arch::ShadowCallStackBacktrace& shadow_call_stack,
                                ktl::optional<uintptr_t> interrupt_pc) {
  Context();

  // For either kind of backtrace, the interrupt_pc is a special frame #0 if
  // it's present.  It gets printed after other messages about the backtrace as
  // a whole, just as if it were stack.front() in a case with no interrupt_pc.
  auto backtrace = [configured_max =
                        BootOptions::Get() ? BootOptions::Get()->phys_backtrace_max : 0,  //
                    this, interrupt_pc](const auto& stack, const char* which) PHYS_SINGLETHREAD {
    Printf("%s: Backtrace (via %s)%s%s\n", name_, which, stack.empty() ? " is empty!" : ":",
           (stack.empty() && interrupt_pc) ? "  Only the interrupted PC is available:" : "");
    if (interrupt_pc) {
      BackTraceFrame(0, *interrupt_pc, true);
      BackTrace(stack, 1, configured_max);
    } else {
      BackTrace(stack, 0, configured_max);
    }
  };

  backtrace(frame_pointers, "frame pointers");

  if (BootShadowCallStack::kEnabled) {
    backtrace(shadow_call_stack, "shadow call stack");
  }
}

void Symbolize::PrintStack(uintptr_t sp, ktl::optional<size_t> max_size_bytes) {
  const size_t configured_max =
      BootOptions::Get() ? BootOptions::Get()->phys_print_stack_max : 1024;
  auto maybe_dump_stack = [max = max_size_bytes.value_or(configured_max), sp,
                           this](const auto& stack) -> bool {
    if (!stack.boot_stack.IsOnStack(sp)) {
      return false;
    }
    Printf("%s: Partial dump of %.*s stack at [%p, %p):\n", name_,
           static_cast<int>(stack.name.size()), stack.name.data(), &stack.boot_stack,
           &stack.boot_stack + 1);
    ktl::span whole(reinterpret_cast<const uint64_t*>(stack.boot_stack.stack),
                    sizeof(stack.boot_stack.stack) / sizeof(uint64_t));
    const uintptr_t base = reinterpret_cast<uintptr_t>(whole.data());
    ktl::span used = whole.subspan((sp - base) / sizeof(uint64_t));
    hexdump(used.data(), ktl::min(max, used.size_bytes()));
    return true;
  };

  if (ktl::none_of(stacks_.begin(), stacks_.end(), maybe_dump_stack)) {
    Printf("%s: Stack pointer is outside expected bounds:", name_);
    for (const auto& stack : stacks_) {
      Printf(" [%p, %p) ", &stack.boot_stack, &stack.boot_stack + 1);
    }
    Printf("\n");
  }
}

bool Symbolize::IsOnStack(uintptr_t sp) const {
  return ktl::any_of(stacks_.begin(), stacks_.end(),
                     [sp](auto&& stack) { return stack.boot_stack.IsOnStack(sp); });
}

arch::ShadowCallStackBacktrace Symbolize::GetShadowCallStackBacktrace(uintptr_t scsp) const {
  arch::ShadowCallStackBacktrace backtrace;
  for (const auto& stack : shadow_call_stacks_) {
    backtrace = stack.boot_stack.BackTrace(scsp);
    if (!backtrace.empty()) {
      break;
    }
  }
  return backtrace;
}

void Symbolize::PruneFromMainModule(uint64_t start, uint64_t end) {
  // Modify the ZeroFillSegment or DataWithZeroFillSegment to prune the range.
  auto adjust_memsz = [start, end, this](auto& segment) {
    [[maybe_unused]] auto abs = segment.vaddr() + main_module_->load_bias();
    ZX_DEBUG_ASSERT(start >= abs);
    ZX_DEBUG_ASSERT(end >= start);
    ZX_DEBUG_ASSERT(end - abs <= segment.memsz());
    auto rel_end = static_cast<size_type>(start - main_module_->load_bias());
    size_type new_memsz = rel_end - segment.vaddr();
    ZX_DEBUG_ASSERT(new_memsz >= segment.filesz());
    PruneSegment(segment, new_memsz);
  };

  // Adjusting the last segment in LoadInfo is actually done by removing the
  // existing segment and then adding back its replacement.
  auto& load_info = const_cast<ElfImage*>(main_module_)->load_info();
  auto segment = load_info.RemoveLastSegment();
  ktl::visit(adjust_memsz, segment);
  auto diag = PanicDiagnostics(name_);
  load_info.AddSegment(diag, segment);
  if (!BootOptions::Get() || BootOptions::Get()->phys_verbose) {
    // Reset the symbolizer markup for the new bounds.
    ContextAlways();
  }
}

#ifndef __i386__

void Symbolize::PrintRegisters(const PhysExceptionState& exc) {
  Printf("%s: Registers stored at %p: {{{hexdict:", name_, &exc);

// TODO(https://fxbug.dev/42172753): Replace with a hexdict abstraction from
// libsymbolizer-markup.
#if defined(__aarch64__)

  for (size_t i = 0; i < ktl::size(exc.regs.r); ++i) {
    if (i % 4 == 0) {
      Printf("\n%s: ", name_);
    }
    Printf("  %sX%zu: 0x%016" PRIx64, i < 10 ? " " : "", i, exc.regs.r[i]);
  }
  Printf("  X30: 0x%016" PRIx64 "\n", exc.regs.lr);
  Printf("%s:    SP: 0x%016" PRIx64 "   PC: 0x%016" PRIx64 " SPSR: 0x%016" PRIx64 "\n", name_,
         exc.regs.sp, exc.regs.pc, exc.regs.cpsr);
  Printf("%s:   ESR: 0x%016" PRIx32 "  FAR: 0x%016" PRIx64 "\n", name_, exc.exc.arch.u.arm_64.esr,
         exc.exc.arch.u.arm_64.far);

#elif defined(__riscv)

  Printf("\n%s:   PC: 0x%016" PRIx64 "  RA: 0x%016" PRIx64 "  SP: 0x%016" PRIx64
         "  GP: 0x%016" PRIx64 "\n",
         name_, exc.regs.pc, exc.regs.ra, exc.regs.sp, exc.regs.gp);
  Printf("%s:   TP: 0x%016" PRIx64 "  T0: 0x%016" PRIx64 "  T1: 0x%016" PRIx64 "  T2: 0x%016" PRIx64
         "\n",
         name_, exc.regs.tp, exc.regs.t0, exc.regs.t1, exc.regs.t2);
  Printf("%s:   S0: 0x%016" PRIx64 "  S1: 0x%016" PRIx64 "  A0: 0x%016" PRIx64 "  A1: 0x%016" PRIx64
         "\n",
         name_, exc.regs.s0, exc.regs.s1, exc.regs.a0, exc.regs.a1);
  Printf("%s:   A2: 0x%016" PRIx64 "  A3: 0x%016" PRIx64 "  A4: 0x%016" PRIx64 "  A5: 0x%016" PRIx64
         "\n",
         name_, exc.regs.a2, exc.regs.a3, exc.regs.a4, exc.regs.a5);
  Printf("%s:   A6: 0x%016" PRIx64 "  A7: 0x%016" PRIx64 "  S2: 0x%016" PRIx64 "  S3: 0x%016" PRIx64
         "\n",
         name_, exc.regs.a6, exc.regs.a7, exc.regs.s2, exc.regs.s3);
  Printf("%s:   S4: 0x%016" PRIx64 "  S5: 0x%016" PRIx64 "  S6: 0x%016" PRIx64 "  S7: 0x%016" PRIx64
         "\n",
         name_, exc.regs.s4, exc.regs.s5, exc.regs.s6, exc.regs.s7);
  Printf("%s:   S8: 0x%016" PRIx64 "  S9: 0x%016" PRIx64 " S10: 0x%016" PRIx64 " S11: 0x%016" PRIx64
         "\n",
         name_, exc.regs.s8, exc.regs.s9, exc.regs.s10, exc.regs.s11);
  Printf("%s:   T3: 0x%016" PRIx64 "  T4: 0x%016" PRIx64 "  T5: 0x%016" PRIx64 "  T6: 0x%016" PRIx64
         "\n",
         name_, exc.regs.t3, exc.regs.t4, exc.regs.t6, exc.regs.t6);
  Printf("%s:   SCAUSE: 0x%016" PRIx64 "  STVAL: 0x%016" PRIx64 "\n", name_,
         exc.exc.arch.u.riscv_64.cause, exc.exc.arch.u.riscv_64.tval);

#elif defined(__x86_64__)

  const auto& exc_arch = exc.exc.arch.u.x86_64;
  Printf("\n%s:  RAX: 0x%016" PRIx64 " RBX: 0x%016" PRIx64 " RCX: 0x%016" PRIx64
         " RDX: 0x%016" PRIx64 "\n",
         name_, exc.regs.rax, exc.regs.rbx, exc.regs.rcx, exc.regs.rdx);
  Printf("%s:  RSI: 0x%016" PRIx64 " RDI: 0x%016" PRIx64 " RBP: 0x%016" PRIx64 " RSP: 0x%016" PRIx64
         "\n",
         name_, exc.regs.rsi, exc.regs.rdi, exc.regs.rbp, exc.regs.rsp);
  Printf("%s:   R8: 0x%016" PRIx64 "  R9: 0x%016" PRIx64 " R10: 0x%016" PRIx64 " R11: 0x%016" PRIx64
         "\n",
         name_, exc.regs.r8, exc.regs.r9, exc.regs.r10, exc.regs.r11);
  Printf("%s:  R12: 0x%016" PRIx64 " R13: 0x%016" PRIx64 " R14: 0x%016" PRIx64 " R15: 0x%016" PRIx64
         "\n",
         name_, exc.regs.r12, exc.regs.r13, exc.regs.r14, exc.regs.r15);
  Printf("%s:  RIP: 0x%016" PRIx64 " RFLAGS: 0x%08" PRIx64 " FS.BASE: 0x%016" PRIx64
         " GS.BASE: 0x%016" PRIx64 "\n",
         name_, exc.regs.rip, exc.regs.rflags, exc.regs.fs_base, exc.regs.gs_base);
  Printf("%s:   V#: %" PRIu64 "  ERR: %#" PRIx64 "  CR2: 0x%016" PRIx64 "\n", name_,
         exc_arch.vector, exc_arch.err_code, exc_arch.cr2);

#else

#warning "need register code dump for this architecture"

#endif

  Printf("%s: }}}\n", name_);
}

void Symbolize::PrintException(uint64_t vector, const char* vector_name,
                               const PhysExceptionState& exc) {
  // To avoid re-entry cascades from exceptions during the steps of this
  // function, maintain a progress indicator.
  enum Progress : unsigned int {
    kNotInUse,
    kEntered,
    kChecked,
    kVector,
    kContext,
    kRegs,
    kFp,
    kScs,
    kPrintBt,
    kStack,
    kStepLimit,
  };
  constexpr unsigned int kSteps = kStepLimit - 1;
  static Progress gProgress = kNotInUse;

  // `return_after(kJustDid)` returns false normally, and returns true if we
  // should bail out early because we've already gotten this far and re-entered
  // without getting farther.
  constexpr auto return_after = [](Progress step) -> bool {
    if (gProgress < step) [[likely]] {
      ZX_ASSERT_MSG(gProgress == step - 1, ": Hit return_after(%u) with gProgress=%u", step,
                    gProgress);
      gProgress = step;
      return false;
    }

    if (gProgress > step) {
      // This is a re-entry, but we haven't yet gotten as far as the last entry
      // got, so keep going to report details about the re-entry.
      printf(
          "PrintException reached step %u of %u on re-entry after previously reaching step %u.\n",
          step, kSteps, gProgress);
      return false;
    }

    // This is a re-entry after not getting farther than this last time.  So it
    // could be a cascade of repeated re-entry that would continue forever if
    // we attempted to go past the step just completed.  Don't even attempt a
    // printf here before recording the first step, so we identify a re-entry
    // that's just from printf on the early-return paths above for later steps.
    if (step != kEntered) {
      printf("PrintException truncated after %u of %u steps in re-entry.\n", step, kSteps);
    }
    return true;
  };

  // The kChecked step just makes sure we won't even try the printf in the
  // early-return case if we re-entered from that very printf at kChecked.
  if (return_after(kEntered) || return_after(kChecked)) {
    return;
  }

  Printf("%s: exception vector %s (%#" PRIx64 ")\n", Symbolize::name_, vector_name, vector);
  if (return_after(kVector)) {
    return;
  }

  // Always print the context, even if it was printed earlier.
  context_done_ = false;
  Context();
  if (return_after(kContext)) {
    return;
  }

  PrintRegisters(exc);
  if (return_after(kRegs)) {
    return;
  }

  // Collect each kind of backtrace if possible.
  FramePointerBacktrace fp_backtrace;
  arch::ShadowCallStackBacktrace scs_backtrace;

  fp_backtrace = FramePointerBacktrace::BackTrace(exc.fp());
  if (return_after(kFp)) {
    return;
  }

  uint64_t scsp = exc.shadow_call_sp();
  scs_backtrace = boot_shadow_call_stack.BackTrace(scsp);
  if (scs_backtrace.empty()) {
    scs_backtrace = phys_exception_shadow_call_stack.BackTrace(scsp);
  }
  if (return_after(kScs)) {
    return;
  }

  // Print whatever we have.
  PrintBacktraces(fp_backtrace, scs_backtrace, exc.pc());
  if (return_after(kPrintBt)) {
    return;
  }

  PrintStack(exc.sp());
  if (return_after(kStack)) {
    return;
  }

  gProgress = kNotInUse;
}

void PrintPhysException(uint64_t vector, const char* vector_name, const PhysExceptionState& regs) {
  if (gSymbolize) {
    gSymbolize->PrintException(vector, vector_name, regs);
  }
}

#endif  // !__i386__
