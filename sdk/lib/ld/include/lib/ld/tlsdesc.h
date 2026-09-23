// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TLSDESC_H_
#define LIB_LD_TLSDESC_H_

#include <lib/arch/asm.h>

// These declarations relate to the TLSDESC runtime ABI.  While the particular
// ABI details are specific to each machine, they all fit a common pattern.
//
// The R_*_TLSDESC relocation type directs dynamic linking to fill in a special
// pair of adjacent GOT slots.  The first slot is unfilled at link time and
// gets the PC of a special function provided by the dynamic linking runtime.
// For each TLS reference, the compiler generates an indirect call via this GOT
// slot.  The compiler also passes the address of its GOT slot to the function.
//
// This is a normal indirect call at the machine level.  However, it uses its
// own bespoke calling convention specified in the psABI for each machine
// rather than the standard C/C++ calling convention.  The convention for each
// machine is similar: the use of the return address register and/or stack is
// normal; one or two registers are designated for the argument (GOT address),
// return value, and scratch; all other registers are preserved by the call,
// except the condition codes.  The return value is a signed offset from the
// psABI-specified thread-pointer register.  Notably, it's expected to be added
// to the thread pointer to yield a valid pointer or nullptr for undefined weak
// symbol references, so it may be a difference of unrelated pointers to reach
// a heap address not near the thread pointer or to reach zero for nullptr.
//
// This makes the impact of the runtime call on code generation very minimal.
// The runtime implementation both can refer to the value stored in the GOT
// slot by dynamic linking and can in theory dynamically update both slots to
// lazily redirect to a different runtime entry point and argument data.
//
// The relocation's symbol and addend are meant to apply to the second GOT slot
// of the pair.  (For DT_REL format, the addend is stored in place there.)
// When dynamic linking chooses an entry point to store into the first GOT slot
// it also chooses the value to store in the second slot, which is some kind of
// offset or address that includes the addend and symbol value calculations.

// This enumerates the canonical entry points provided by a startup dynamic
// linker for the different kinds of TLSDESC resolution needed in the simple
// static TLS cases.  These entry points and their semantics are not part of
// the TLSDESC ABI, they are just implementation details provided by library
// code that can be used in a dynamic linker implementation.  Each of these
// canonical implementations has a corresponding definition of what must be
// stored in the second GOT slot, below called the "argument" (though each
// psABI calling convention actually passes the address of the first GOT slot
// in a register, from which the adjacent slot must be loaded).
//
//  * kStatic handles a fixed offset from the thread pointer.  The argument is
//    that final offset, which already includes any relocation addend as well
//    as the symbol's offset into the PT_TLS and the definer's TLS block's
//    offset from the thread pointer.  The hook returns that offset to be added
//    to the thread pointer: the same offset in every thread so the TLS block
//    offset must be statically assigned before new threads can be created.
//    The dynamic linker must perform the static TLS layout to assign
//    correctly-aligned and sized regions relative to the thread pointer
//    (<lib/elfldltl/tls-layout.h> handles the details of that arithmetic), and
//    then install this entry point alongside the relocation's symbol's
//    definer's TLS block offset plus the symbol value and relocation addend.
//
//  * kUndefinedWeak handles an undefined weak STT_TLS symbol where there was
//    no relocation addend.  The argument is ignored.  The hook returns the
//    thread pointer negated, so the address resolves as zero (a null pointer)
//    when the thread pointer is added to the return value.  The dynamic linker
//    need only store the entry point in the GOT.
//
//  * kUndefinedWeakAddend handles an undefined weak STT_TLS symbol where there
//    was a nonzero relocation addend.  The argument is the addend.  The hook
//    returns the thread pointer negated plus the addend, to yield the same
//    result as applying the addend to a byte pointer that was null.  The
//    dynamic linker must store the entry point and addend in the GOT.
//
#define LD_TLSDESC_STATIC_HOOKS(Macro) \
  Macro(kStatic) Macro(kUndefinedWeak) Macro(kUndefinedWeakAddend)

#define LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC 0x00534c54  // LE 'TLS\0'

#ifdef __ASSEMBLER__  // clang-format off

// In assembly, `.tlsdesc.lsda kName` with one of the LD_TLSDESC_STATIC_HOOKS
// names described above yields a `.cfi_lsda DW_EH_PE_absptr, <magic number>`
// that will give the current FDE an LSDA value magic corresponding to the
// ld::TlsdescStaticHook C++ enum defined below.  The library's implementation
// code uses these to aid in locating entry points in the stub dynamic linker
// with no ELF symbols.
.macro .tlsdesc.lsda name
  .cfi_lsda 0, .Ltlsdesc.hook.\name // 0 is DW_EH_PE_absptr encoding: no reloc.
.endm
  // This bit just gets all the .Ltlsdesc.hook.\name values defined.
  // These local symbols will usually be elided from the symbol table.
  .macro _.tlsdesc.hook.const name
    .Ltlsdesc.hook.\name = LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC + \@ - 1
  .endm
#define _LD_TLSDESC_HOOK_CONST(name) _.tlsdesc.hook.const name;
  LD_TLSDESC_STATIC_HOOKS(_LD_TLSDESC_HOOK_CONST)
#undef _LD_TLSDESC_HOOK_CONST
  .purgem _.tlsdesc.hook.const

// The pseudo-op `.tlsdesc.cfi`, given `.cfi_startproc` initial state,
// resets CFI to indicate the special ABI for the R_*_TLSDESC callback
// function on this machine.
//
// Other conveniences are defined specific to each machine.

// This is like .function, but adds some appropriate default arguments
// for TLSDESC resolver functions, and does .tlsdesc.cfi.
.macro .tlsdesc.function name, scope=global, retain=, align=
  .function \name, scope=\scope, retain=\retain, align=\align
    .tlsdesc.cfi
.endm

#if defined(__aarch64__)

.macro .tlsdesc.cfi
  // Almost all registers are preserved from the caller.  The integer set does
  // not include x30 (LR) or SP, which .cfi_startproc covered.
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
.endm

// On AArch64 ILP32, GOT entries are 4 bytes, not 8.
# ifdef _LP64
tlsdesc_r0 .req x0
tlsdesc_r1 .req x1
tlsdesc_r2 .req x2
tlsdesc.value_offset = 8
#define tlsdesc_uxtw(reg, shift) reg, lsl shift  // For second register in ldr.
# else
tlsdesc_r0 .req w0
tlsdesc_r1 .req w1
tlsdesc_r2 .req w2
tlsdesc.value_offset = 4
#define tlsdesc_uxtw(reg, shift) reg, uxtw shift
# endif

#elif defined(__arm__)

.macro .tlsdesc.cfi
  // Almost all registers are preserved from the caller.  The integer set does
  // not include LR or SP, which .cfi_startproc covered.
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
.endm

#elif defined(__riscv)

.macro .tlsdesc.cfi
  // Almost all registers are preserved from the caller.  The integer set does
  // not include sp, which .cfi_startproc covered.
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value

  // The return address is in t0 rather than the usual ra, and preserved there.
  .cfi_return_column t0
.endm

# ifdef _LP64
.macro tlsdesc.load reg, mem
  ld \reg, \mem
.endm
.macro tlsdesc.add rd, r1, r2
  add \rd, \r1, \r2
.endm
.macro tlsdesc.sub rd, r1, r2
  sub \rd, \r1, \r2
.endm
tlsdesc.value_offset = 8
# else
.macro tlsdesc.load reg, mem
  lw \reg, \mem
.endm
.macro tlsdesc.add rd, r1, r2
  addw \rd, \r1, \r2
.endm
.macro tlsdesc.sub rd, r1, r2
  subw \rd, \r1, \r2
.endm
tlsdesc.value_offset = 4
# endif

#elif defined(__x86_64__)

.macro .tlsdesc.cfi
  // Almost all registers are preserved from the caller.  The integer set does
  // not include %rsp, which .cfi_startproc covered.
  .cfi.all_integer .cfi_same_value
  .cfi.all_vectorfp .cfi_same_value
.endm

# ifdef _LP64
#define tlsdesc_ax rax
#define tlsdesc_cx rcx
#define tlsdesc_dx rdx
# else
#define tlsdesc_ax eax
#define tlsdesc_cx ecx
#define tlsdesc_dx edx
# endif
tlsdesc.value_offset = 8  // x86-64 GOT slots are 64 bits even in ILP32.

#else

// Not all machines have TLSDESC support specified in the psABI.

#endif

#else  // clang-format on

#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/symbol.h>
#include <lib/fit/result.h>

#include <bit>
#include <cstddef>
#include <optional>

namespace [[gnu::visibility("hidden")]] ld {

// These are callback functions to be used in the TlsDescGot::function slot
// at runtime.  Though they're declared here as C++ functions with an
// argument, they're actually implemented in assembly code with a bespoke
// calling convention for the argument, return value, and register usage
// that's different from normal functions, so these cannot actually be
// called from C++.  These symbol names are not visible anywhere outside
// the dynamic linking implementation itself and these functions are only
// ever called by compiler-generated TLSDESC references.
using TlsdescCallback = ptrdiff_t(const elfldltl::Elf<>::TlsDescGot<>& got);

extern "C" {

// These are used for undefined weak definitions.  The value slot contains just
// the addend; the first entry-point ignores the addend and is cheaper for a
// zero addend (the most common case), while the second supports an addend.
// The implementation returns the addend minus the thread pointer, such that
// adding the thread pointer back to this offset produces zero with a zero
// addend, and thus nullptr.
extern TlsdescCallback _ld_tlsdesc_runtime_undefined_weak;
extern TlsdescCallback _ld_tlsdesc_runtime_undefined_weak_addend;

// In this minimal implementation used for PT_TLS segments in the static TLS
// set, desc.valueu is always simply a fixed offset from the thread pointer.
// Note this offset might be negative, but it's always handled as uintptr_t to
// ensure well-defined overflow arithmetic.
extern TlsdescCallback _ld_tlsdesc_runtime_static;

}  // extern "C"

// ld::TlsdescRuntime::k* can be used as indices into an array of
// kTlsdescRuntimeCount to cover all the entry points.

enum class TlsdescRuntime {
#define LD_TLSDESC_RUNTIME_CONST(name) name,
  LD_TLSDESC_STATIC_HOOKS(LD_TLSDESC_RUNTIME_CONST)
#undef LD_TLSDESC_RUNTIME_CONST
};

inline constexpr size_t kTlsdescRuntimeCount = []() {
  size_t n = 0;
#define LD_TLSDESC_RUNTIME_MAX(name) ++n;
  LD_TLSDESC_STATIC_HOOKS(LD_TLSDESC_RUNTIME_MAX)
#undef LD_TLSDESC_RUNTIME_MAX
  return n;
}();

// For each hook there is a different uint32_t magic number that will be found
// in its FDE's LSDA.
template <TlsdescRuntime Hook>
inline constexpr uint32_t kTlsdescRuntimeMagic =
    LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC + static_cast<uint32_t>(Hook);

// In the stub dynamic linker, the only FDEs should be for these entry points,
// so each one's LSDA should correspond to one of the TlsdescRuntime indices.
constexpr std::optional<TlsdescRuntime> TlsdescRuntimeFromMagic(uintptr_t magic) {
  if (magic >= LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC &&
      magic < LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC + kTlsdescRuntimeCount) {
    magic -= LD_TLSDESC_STATIC_HOOKS_FIRST_MAGIC;
    return static_cast<TlsdescRuntime>(magic);
  }
  return std::nullopt;
}

// This provides a callable object for resolving TLSDESC relocations.  It can
// be passed directly to elfldltl::MakeSymbolResolver (by reference).
//
// The constructor requires an array of Elf::Addr indexed by the TlsdescRuntime
// enum values (as from ld::RemoteAbiStub), and a load bias applied to those
// addresses.  Individual hooks can be replaced later with SetHook.
//
template <class Elf, elfldltl::ElfMachine Machine>
class StaticTlsDescResolver {
 public:
  using size_type = Elf::size_type;
  using Addr = Elf::Addr;
  using Addend = Elf::Addend;
  using TlsDescGot = Elf::template TlsDescGot<Machine>;
  using RuntimeHooks = std::array<Addr, kTlsdescRuntimeCount>;

  explicit constexpr StaticTlsDescResolver(const RuntimeHooks& hooks, size_type bias = 0) {
    for (size_t i = 0; i < hooks.size(); ++i) {
      hooks_[i] = hooks[i] + bias;
    }
  }

  constexpr Addr GetHook(TlsdescRuntime hook) const { return hooks_[static_cast<size_t>(hook)]; }

  constexpr void SetHook(TlsdescRuntime hook, size_type value) {
    hooks_[static_cast<size_t>(hook)] = value;
  }

  // Handle an undefined weak TLSDESC reference.  There are two special runtime
  // resolvers for this case: one for zero addend, and one for nonzero addend.
  constexpr TlsDescGot operator()(Addend addend) const {
    if (addend == 0) {
      return {.function = GetHook(TlsdescRuntime::kUndefinedWeak)};
    }
    return {
        .function = GetHook(TlsdescRuntime::kUndefinedWeakAddend),
        .value = std::bit_cast<size_type>(addend),
    };
  }

  // Handle a TLSDESC reference to a defined symbol.  The runtime resolver just
  // loads the static TLS offset from the value slot.  When the relocation is
  // applied the addend will be added to the value computed here from the
  // symbol's value and module's PT_TLS offset.
  constexpr fit::result<bool, TlsDescGot> operator()(auto& diag, const auto& defn) const {
    assert(!defn.undefined_weak());
    // Note that defn.uses_static_tls() need not be true: this resolver is only
    // used at all when the module being relocated is in the Initial Exec set,
    // so anything its references resolve to will also be in the Initial Exec
    // set even if it wasn't marked with DF_STATIC_TLS at link time.
    return fit::ok(TlsDescGot{
        .function = GetHook(TlsdescRuntime::kStatic),
        .value = defn.symbol().value + defn.static_tls_bias(),
    });
  }

 private:
  RuntimeHooks hooks_;
};

// This provides a callable object for resolving TLSDESC relocations.  It can
// be passed directly to elfldltl::MakeSymbolResolver (by reference).
//
// This uses the _ld_tlsdesc_runtime_* entry points declared above and provided
// in the library, and so is only appropriate for use within the same module,
// address space, and ABI and where the TLSDESC accesses will be made.
//
// This handles only the static TLS cases, by using defn->static_tls_bias().
// Where dynamic TLS is also possible, a subclass can do something like:
// ```
// using ld::LocalRuntimeTlsDescResolver::operator();
// ...
// fit::result<bool, TlsDescGot> operator()(auto& diag, const auto& defn) const {
//   assert(defn->tls_module_id() != 0);
//   if (IsInStaticTls(defn)) {
//     return ld::LocalRuntimeTlsDescResolver::operator()(diag, defn);
//   }
//   return fit::ok(TlsDescGot{...});
// }
// ```
struct LocalRuntimeTlsDescResolver {
  using size_type = elfldltl::Elf<>::size_type;
  using Addr = elfldltl::Elf<>::Addr;
  using Addend = elfldltl::Elf<>::Addend;
  using TlsDescGot = elfldltl::Elf<>::TlsDescGot<>;

  // Handle an undefined weak TLSDESC reference.  There are special runtime
  // resolvers for this case: one for zero addend, and one for nonzero addend.
  TlsDescGot operator()(Addend addend) const {
    if (addend == 0) {
      return {.function = kRuntimeUndefinedWeak};
    }
    return {
        .function = kRuntimeUndefinedWeakAddend,
        .value = std::bit_cast<size_type>(addend),
    };
  }

  // Handle a TLSDESC reference to a defined symbol.  The runtime resolver just
  // loads the static TLS offset from the value slot.  When the relocation is
  // applied the addend will be added to the value computed here from the
  // symbol's value and module's PT_TLS offset.
  fit::result<bool, TlsDescGot> operator()(auto& diag, const auto& defn) const {
    assert(!defn.undefined_weak());
    return fit::ok(TlsDescGot{
        .function = kRuntimeStatic,
        .value = defn.symbol().value + defn.static_tls_bias(),
    });
  }

  static inline const Addr kRuntimeStatic{
      reinterpret_cast<uintptr_t>(_ld_tlsdesc_runtime_static),
  };
  static inline const Addr kRuntimeUndefinedWeak{
      reinterpret_cast<uintptr_t>(_ld_tlsdesc_runtime_undefined_weak),
  };
  static inline const Addr kRuntimeUndefinedWeakAddend{
      reinterpret_cast<uintptr_t>(_ld_tlsdesc_runtime_undefined_weak_addend),
  };
};

}  // namespace ld

#endif  // __ASSEMBLER__

#endif  // LIB_LD_TLSDESC_H_
