// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_

#include <lib/arch/backtrace.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/preallocated-vector.h>
#include <lib/symbolizer-markup/writer.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/assert.h>

#include <ktl/algorithm.h>
#include <ktl/byte.h>
#include <ktl/declval.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <phys/main.h>
#include <phys/stack.h>
#include <phys/stdio.h>

// FileMarkupWriter{{stdout}} et al can be used with SymbolizerContext et al.
using FileMarkupWriter = symbolizer_markup::Writer<CallableFile>;

class ElfImage;
struct PhysExceptionState;
class Symbolize;

// The Symbolize instance registered by MainSymbolize.
extern class Symbolize* gSymbolize;

class Symbolize {
 public:
  template <class BootStackType>
  struct Stack {
    BootStackType& boot_stack;
    std::string_view name;
  };

  struct IsOnStackFunction {
    bool operator()(const void* ptr) const {
      return gSymbolize && gSymbolize->IsOnStack(reinterpret_cast<uintptr_t>(ptr));
    }
  };
  using FramePointerBacktrace = arch::FramePointerBacktrace<IsOnStackFunction>;

  using ModuleList = elfldltl::PreallocatedVector<const ElfImage*>;

  // This is the type of the generated __cfi_check function in a module.
  //
  // TODO(https://fxbug.dev/432080124): For consistency with other type aliases,
  // this attribute should be placed after `CfiCheckFunction` and before the `=`,
  // but the parsing mechanics aren't set up yet to support this for `cfi_unchecked_callee`.
  // Relocate the attribute once it's addressed upstream.
  using CfiCheckFunction = void(uint64_t key, void* entry, const void* diag_data)
      [[clang::cfi_unchecked_callee]];

  Symbolize() = delete;
  Symbolize(const Symbolize&) = delete;

  explicit Symbolize(const char* name, FILE* f = stdout)
      : name_{name}, output_{f}, writer_{{output_}} {}

  const char* name() const { return name_; }

  void set_name(const char* new_name) { name_ = new_name; }

  auto modules() const { return modules_.as_span(); }

  const ElfImage* module_for_vaddr_range(uintptr_t vaddr, size_t len) const;

  // Return the ELF build ID note for the currently executing module (i.e., the
  // first module to call OnLoad or the last module to call OnHandoff).
  elfldltl::ElfNote build_id() const;

  // This reads the existing modules to the new storage and then
  // replaces it as the storage to be used by future OnLoad calls.
  void ReplaceModulesStorage(ModuleList modules);

  void set_stacks(ktl::span<const Stack<BootStack>> stacks) { stacks_ = stacks; }

  void set_shadow_call_stacks(ktl::span<const Stack<BootShadowCallStack>> stacks) {
    shadow_call_stacks_ = stacks;
  }

  bool IsOnStack(uintptr_t sp) const;

  arch::ShadowCallStackBacktrace GetShadowCallStackBacktrace(
      uintptr_t scsp = arch::GetShadowCallStackPointer()) const;

  FramePointerBacktrace GetFramePointerBacktrace(
      const arch::CallFrame* fp = static_cast<arch::CallFrame*>(__builtin_frame_address(0))) const {
    return FramePointerBacktrace::BackTrace(fp);
  }

  // Print the contextual markup elements describing each loaded module.
  void ContextAlways(FILE* log = nullptr);

  // Same, but idempotent: the first call prints and others do nothing.
  void Context();

  // Print one module's context if Context() has already printed.
  void ModuleContext(const ElfImage& loaded, unsigned int module_id, bool physical = false);

  // Adds the new module to the list.  If Context() has run, then emit context
  // for the new module.
  void OnLoad(const ElfImage& loaded);

  // Registers the next, just-handed-of-to module as the currently executing
  // one.
  void OnHandoff(ElfImage& next);

  void LogHandoff(ktl::string_view name, uintptr_t entry_pc);

  // Print the presentation markup element for one frame of a backtrace.
  void BackTraceFrame(unsigned int n, uintptr_t pc, bool interrupt = false);

  // Print a backtrace, ensuring context has been printed beforehand.
  // This takes any container of uintptr_t, so FramePointer works.
  template <typename T>
  PHYS_SINGLETHREAD void BackTrace(const T& pcs, unsigned int n, unsigned int max) {
    Context();
    for (uintptr_t pc : pcs) {
      if (max != 0 && n >= max) {
        writer_.Prefix(name_)
            .Literal("Backtrace truncated before frame #")
            .DecimalDigits(n)
            .Newline();
        break;
      }
      BackTraceFrame(n++, pc);
    }
  }

  // Print both flavors of backtrace together.  If the optional interrupt_pc
  // argument is supplied, then it's inserted as frame 0 and marked as exact PC
  // (whereas all frame-pointer and shadow-call-stack frames are marked as RA).
  PHYS_SINGLETHREAD void PrintBacktraces(const FramePointerBacktrace& frame_pointers,
                                         const arch::ShadowCallStackBacktrace& shadow_call_stack,
                                         ktl::optional<uintptr_t> interrupt_pc = ktl::nullopt);

  // Print the trigger markup element for a dumpfile.
  void DumpFile(ktl::string_view announce, size_t size_bytes, ktl::string_view sink_name,
                ktl::string_view vmo_name, ktl::string_view vmo_name_suffix = "");

  // Dump some stack up to the SP.
  PHYS_SINGLETHREAD void PrintStack(uintptr_t sp,
                                    ktl::optional<size_t> max_size_bytes = ktl::nullopt);

  // Print out register values.
  PHYS_SINGLETHREAD void PrintRegisters(const PhysExceptionState& regs);

  // Print out useful details at an exception.
  PHYS_SINGLETHREAD void PrintException(uint64_t vector, const char* vector_name,
                                        const PhysExceptionState& regs);

  static void CallCfiSlowpath(uint64_t key, void* entry, const void* diag_data = nullptr,
                              void* caller = __builtin_return_address(0));

  // The arguments must be a subrange of the main module's final segment's
  // runtime vaddr bounds (i.e. a range of data + bss addresses).  That tail
  // should no longer be considered part of the segment's or module's bounds.
  void PruneFromMainModule(uint64_t start, uint64_t end);

 protected:
  // TODO(https://fxbug.dev/432080124): For consistency with other type aliases,
  // this attribute should be placed after `CallCfiSlowpathFunction` and before the `=`,
  // but the parsing mechanics aren't set up yet to support this for `cfi_unchecked_callee`.
  // Relocate the attribute once it's addressed upstream.
  using CallCfiSlowpathFunction = void(Symbolize* main_symbolize, uint64_t key, void* entry,
                                       const void* diag_data, void* caller)
      [[clang::cfi_unchecked_callee]];

  void set_main_module(const ElfImage& main) {
    ZX_DEBUG_ASSERT(!main_module_);
    main_module_ = &main;
  }

  void set_cfi_slowpath(CallCfiSlowpathFunction* cfi_slowpath, CfiCheckFunction* main_cfi_check);

 private:
  [[gnu::format(printf, 2, 3)]] void Printf(const char* fmt, ...);

  void AddModule(const ElfImage* module);

  const char* name_;
  FILE* output_;
  ModuleList modules_;
  ktl::span<const Stack<BootStack>> stacks_;
  ktl::span<const Stack<BootShadowCallStack>> shadow_call_stacks_;
  FileMarkupWriter writer_;
  // The currently executing module, set on the first call to LoadModule() or
  // the last to call OnHandoff().
  const ElfImage* main_module_ = nullptr;
  bool context_done_ = false;
  CallCfiSlowpathFunction* cfi_slowpath_ = nullptr;
};

// MainSymbolize represents the singleton Symbolize instance to be used by the
// current program. On construction, it regsters itself as `gSymbolize` and
// emits symbolization markup context.
class MainSymbolize : public Symbolize {
 public:
  explicit MainSymbolize(const char* name, FILE* f = stdout);

  const ElfImage& self() const { return *self_; }

  void set_self(const ElfImage* self);

  void EnableModuleLoading(ModuleList modules) {
    ReplaceModulesStorage(ktl::move(modules));
    HandleCfiSlowpath();
  }

 private:
  static void CallCfiSlowpath(Symbolize* main_symbolize, uint64_t key, void* entry,
                              const void* diag_data, void* caller);
  void CfiSlowpath(uint64_t key, void* entry, const void* diag_data, void* caller);

  // This is called when cross-DSO CFI checking is going to be supported.
  void HandleCfiSlowpath();

  const ElfImage* self_ = nullptr;
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_
