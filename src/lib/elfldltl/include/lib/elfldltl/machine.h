// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MACHINE_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MACHINE_H_

#include <cstdint>
#include <optional>
#include <type_traits>

#include "constants.h"
#include "layout.h"

namespace elfldltl {

// This is specialized to give some machine-specific details on ABI.
// This is more about calling conventions than anything directly to do
// with ELF, but it's a common part of what's entailed in program loading.
template <ElfMachine Machine = ElfMachine::kNative>
struct AbiTraits;

// This is the prototypical specialization that serves to document the
// AbiTraits API.  It does not correspond to an actual machine ABI per se, but
// does provide a common base class for specializations defined below.
template <>
struct AbiTraits<ElfMachine::kNone> {
  // The minimum alignment to which the machine stack pointer must be kept.
  // 16-byte alignment is a common ABI requirement across several machines.
  template <typename SizeType = uintptr_t>
  static constexpr SizeType kStackAlignment = 16;

  // Given the base address and size of a machine stack block, compute the
  // initial SP value for using a psABI C function as an entry point address.
  template <typename SizeType>
  static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
    // Stacks grow down on most machines.
    return (base + size) & -kStackAlignment<SizeType>;
  }
};

// AArch64 has simple 16-byte stack alignment.
template <>
struct AbiTraits<ElfMachine::kAarch64> : public AbiTraits<ElfMachine::kNone> {};

// ARM (AArch32) has only 8-byte stack alignment.
template <>
struct AbiTraits<ElfMachine::kArm> {
  template <typename SizeType = uint32_t>
  static constexpr SizeType kStackAlignment = 8;

  template <typename SizeType = uint32_t>
  static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
    return (base + size) & -kStackAlignment<SizeType>;
  }
};

// x86-64 requires exactly 8 below 16-byte alignment for the entry SP,
// consistent with the CALL instruction pushing the return address on
// the stack when it was 16-byte-aligned at the call site.
template <>
struct AbiTraits<ElfMachine::kX86_64> : public AbiTraits<ElfMachine::kNone> {
  template <typename SizeType>
  static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
    return AbiTraits<ElfMachine::kNone>::InitialStackPointer(base, size) - 8;
  }
};

// i386 requires exactly 4 below 16-byte alignment for the entry SP,
// consistent with the CALL instruction pushing the return address on
// the stack when it was 16-byte-aligned at the call site.
template <>
struct AbiTraits<ElfMachine::k386> : public AbiTraits<ElfMachine::kNone> {
  template <typename SizeType>
  static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
    return AbiTraits<ElfMachine::kNone>::InitialStackPointer(base, size) - 4;
  }
};

// RISCV has simple 16-byte stack alignment.
template <>
struct AbiTraits<ElfMachine::kRiscv> : public AbiTraits<ElfMachine::kNone> {};

// This is specialized to give the machine-specific details on relocation.
template <ElfMachine Machine = ElfMachine::kNative>
struct RelocationTraits;

// This is the prototypical specialization that serves to document the
// RelocationTraits API.  It does not correspond to an actual machine
// format; actual files with EM_NONE should not be produced or consumed
// using these relocation types.  But this can be used in unit tests.
template <>
struct RelocationTraits<ElfMachine::kNone> {
  // This lists a small subset of the relocation type codes for the machine.
  // This doesn't define each per-machine type with its own canonical name.
  // Instead it only the types used by modern dynamic linking ABIs.  Each of
  // the few types actually supported for dynamic linking has the same
  // semantics across machines, but each machine its own different name and
  // type code for each one.  This type uses a uniform set of names for these,
  // but with the actual type values each machine encodes in Elf::Rel::type().
  // The semantics associated with each type name are described below.
  //
  // In pseudo-code expressions below, these variables are used:
  //  * `Base` is the load bias of the relocated module (i.e. the difference
  //    between its runtime load address and its first PT_LOAD's p_vaddr).
  //  * `SymbolBase` is the load bias of the module defining this symbol.
  //  * `SymbolValue` is the st_value of the defining module's symbol.
  //  * `Addend` is r_addend or equivalent extracted (signed) value.
  //
  // The datum being relocated is located at Base + r_offset.

  enum class Type : uint32_t {
    // This type should never appear but has always been assigned with value
    // zero in every ABI.  Historically some linkers have occasionally produced
    // filler entries with this type that should be ignored.
    kNone,

    // These types can touch anywhere in initialized data or the GOT.
    // Theoretically they might not always be aligned in some ABIs, but this
    // implementation only supports naturally aligned relocation targets.
    // Misaligned targets cannot arise from standard C/C++ initializers, since
    // address-holding types require natural alignment in every ABI.
    kRelative,  // Base + Addend
    kAbsolute,  // SymbolBase + SymbolValue + Addend

    // GOT types do not use the addend.
    kPlt,  // SymbolBase + SymbolValue

    // This is a GOT type that stores the TLS module ID of the defining module.
    // The GOT slot is used in arguments to the ABI's runtime callback to
    // resolve thread-local references in the GD/LD TLS model.
    kTlsModule,

    // TLS "address" types use SymbolValue + Addend as the value stored.
    kTlsAbsolute,  // Relative to the thread pointer (static TLS).
    kTlsRelative,  // Relative to the symbol-defining module's TLS block.
  };

  // These types are not available on every machine and so are not included in
  // the enum.  Instead, they are defined as constexpr std::optional<uint32_t>.

  // This is like kAbsolute but without the addend, so when not doing lazy PLT
  // fixup it's exactly the same as kPlt: SymbolBase + SymbolValue.  Some
  // machines don't have a separate GOT type at all and just use kAbsolute.
  static constexpr std::optional<uint32_t> kGot = std::nullopt;

  // TLSDESC is the only type that doesn't store exactly one word.
  // It stores into two adjacent GOT slots at Base + r_offset.
  //
  // This is the modern alternative to using kTlsModule + kTlsRelative; it
  // performs better at runtime and so is always the preferred form for the
  // compiler to generate.  The first slot gets filled at runtime with the PC
  // address of a callback function.  Compiled code calls this using a special
  // calling convention that passes in the address of the GOT slots and gets
  // back a per-thread location or offset (details vary by machine, but it's a
  // bespoke convention that minimizes register spills for efficiency).  The
  // addend applies to, and for REL format is stored in, the *second* slot.
  // The runtime setup updates that slot to hold state used by its callback.
  static constexpr std::optional<uint32_t> kTlsDesc = std::nullopt;
};

// Specialization for AArch64.  TODO(mcgrathr): Different types used for same
// things on ILP32, vs ELFCLASS32 LP64 wrt GOT size et al: LP64 reloc all types
// >255, don't fit in Elf32::Rel::r_info.
template <>
struct RelocationTraits<ElfMachine::kAarch64> {
  enum class Type : uint32_t {
    kNone = 0,            // R_AARCH64_NONE
    kRelative = 1027,     // R_AARCH64_RELATIVE
    kAbsolute = 257,      // R_AARCH64_ABS64
    kPlt = 1026,          // R_AARCH64_JUMP_SLOT
    kTlsAbsolute = 1030,  // R_AARCH64_TLS_TPREL64
    kTlsRelative = 1029,  // R_AARCH64_TLS_DTPREL64
    kTlsModule = 1028,    // R_AARCH64_TLS_DTPMOD64
  };
  static constexpr std::optional<uint32_t> kGot = 1025;      // R_AARCH64_GLOB_DAT
  static constexpr std::optional<uint32_t> kTlsDesc = 1031;  // R_AARCH64_TLSDESC
};

// Specialization for ARM (AArch32).
template <>
struct RelocationTraits<ElfMachine::kArm> {
  enum class Type : uint32_t {
    kNone = 0,          // R_ARM_NONE
    kRelative = 23,     // R_ARM_RELATIVE
    kAbsolute = 2,      // R_ARM_ABS32
    kPlt = 22,          // R_ARM_JUMP_SLOT
    kTlsAbsolute = 11,  // R_ARM_TLS_TPOFF32
    kTlsRelative = 9,   // R_ARM_TLS_DTPOFF32
    kTlsModule = 7,     // R_ARM_TLS_DTPMOD32
  };

  static constexpr std::optional<uint32_t> kGot = 21;      // R_ARM_GLOB_DAT
  static constexpr std::optional<uint32_t> kTlsDesc = 13;  // R_ARM_TLS_DESC
};

// Specialization for x86-64.
template <>
struct RelocationTraits<ElfMachine::kX86_64> {
  enum class Type : uint32_t {
    kNone = 0,          // R_X86_64_NONE
    kRelative = 8,      // R_X86_64_RELATIVE
    kAbsolute = 1,      // R_X86_64_64
    kPlt = 7,           // R_X86_64_JUMP_SLOT
    kTlsAbsolute = 18,  // R_X86_64_TPOFF64
    kTlsRelative = 17,  // R_X86_64_DTPOFF64
    kTlsModule = 16,    // R_X86_64_DTPMOD64

  };
  static constexpr std::optional<uint32_t> kGot = 6;       // R_X86_64_GLOB_DAT
  static constexpr std::optional<uint32_t> kTlsDesc = 36;  // R_X86_64_TLSDESC
};

// Specialization for i386.
template <>
struct RelocationTraits<ElfMachine::k386> {
  enum class Type : uint32_t {
    kNone = 0,          // R_386_NONE
    kRelative = 8,      // R_386_RELATIVE
    kAbsolute = 1,      // R_386_64
    kPlt = 7,           // R_386_JUMP_SLOT
    kTlsAbsolute = 37,  // R_386_TPOFF32
    kTlsRelative = 36,  // R_386_DTPOFF32
    kTlsModule = 35,    // R_386_DTPMOD32
  };
  static constexpr std::optional<uint32_t> kGot = 6;       // R_386_GLOB_DAT
  static constexpr std::optional<uint32_t> kTlsDesc = 41;  // R_386_TLS_DESC
};

// Specialization for RISCV.
template <>
struct RelocationTraits<ElfMachine::kRiscv> {
  enum class Type : uint32_t {
    kNone = 0,          // R_RISCV_NONE
    kRelative = 3,      // R_RISCV_RELATIVE
    kAbsolute = 2,      // R_RISCV_64
    kPlt = 5,           // R_RISCV_JUMP_SLOT
    kTlsAbsolute = 11,  // R_RISCV_TPREL64
    kTlsRelative = 9,   // R_RISCV_DTPREL64
    kTlsModule = 7,     // R_RISCV_DTPMOD64
  };

  // RISCV doesn't have a separate GOT type, since the semantics are the same
  // as kAbsolute anyway.
  static constexpr std::optional<uint32_t> kGot = std::nullopt;

  static constexpr std::optional<uint32_t> kTlsDesc = 12;  // R_RISCV_TLSDESC
};

// This is specialized to give the machine-specific details on dynamic linking
// for TLS.  This is only what relocation needs to handle, not the whole
// thread-pointer ABI for the machine.  Each specialization must meet the
// concept TlsTraitsApi<Elf> defined below.
template <ElfApi Elf, ElfMachine Machine>
struct TlsTraitsImpl;

// This defines the API for each TlsTraitsImpl<Elf, Machine> class, given Elf.
//
// _Note:_ An expression like `std::integral_constant<T, Traits::K>{}`
// indicates the requirement for a public member `static constexpr T K = ...;`.
template <class Traits, class Elf = Elf<>>
concept TlsTraitsApi = requires {
  requires ElfApi<Elf>;

  // This is the type of GOT entries in this ABI, usually Elf::Addr.
  typename Traits::GotAddr;

  // Each module in the initial-exec set that has a PT_TLS segment gets
  // assigned an offset from the thread pointer where its PT_TLS block will
  // appear in each thread's static TLS area.  If the main executable has a
  // PT_TLS segment, then it will have module ID 1 and its Local Exec
  // relocations will have been assigned statically by the linker.
  //
  // The psABI sets a starting offset from the thread pointer that the main
  // executable's PT_TLS segment will be assigned.  The actual offset the
  // linker uses is rounded up based on the p_align of that PT_TLS segment.  So
  // the entire block is expected to be aligned such that the thread pointer's
  // value has the maximum alignment of any PT_TLS segment in the static TLS
  // area, and then the linker will align offsets up as necessary.  The Local
  // Exec offset is the offset that the first PT_TLS segment (the executable's
  // if it has one) would be assigned if p_align were 1.
  //
  // Note that this area is always reserved, even the main executable has no
  // PT_TLS and no Local Exec accesses will be made.  The runtime always lays
  // out the thread pointer memory with this space reserved for private uses,
  // and puts the first PT_TLS segment after it.
  { std::integral_constant<typename Elf::size_type, Traits::kTlsLocalExecOffset>{} };

  // If true, TLS offsets from the thread pointer are negative.  Calculations
  // for thread pointer alignment are the same whether offsets are positive or
  // negative: that the first PT_TLS segment (the executable's if it has one)
  // has the offset closest to zero that is aligned to p_align and >= p_memsz.
  { std::bool_constant<Traits::kTlsNegative>{} };

  // If true, the ABI requires that the thread pointer (at offset zero) point
  // to a pointer with the same value as the thread pointer itself.  This is
  // only required on machines like x86 where it hasn't always been trivial to
  // read the thread pointer's value rather than only do a load relative to it.
  { std::bool_constant<Traits::kTpSelfPointer>{} };

  // This bias is subtracted from the offset for a kTlsRelative relocation.
  // It's then added back in again by the `__tls_get_addr` code.  In Local
  // Dynamic cases, there is no kTlsRelative relocation emitted and instead the
  // offset word is filled at link time with this bias subtracted.
  { std::integral_constant<typename Elf::size_type, Traits::kTlsRelativeBias>{} };
};

// This is a convenient way to verify a TlsTraitsApi implementation.
template <ElfApi Elf, TlsTraitsApi<Elf> Traits>
using AsTlsTraitsApi = Traits;

// All uses go through this so any TlsTraitsImpl specialization actually used
// will be verified against the concept requirements.
template <ElfApi Elf = Elf<>, ElfMachine Machine = ElfMachine::kNative>
using TlsTraits = AsTlsTraitsApi<Elf, TlsTraitsImpl<Elf, Machine>>;

// This is an exemplar and recommended starting point for newly-specified TLS
// psABIs.  Specializations for real machines can use this as a base class.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::kNone> {
  using GotAddr = Elf::Addr;
  using size_type = Elf::size_type;

  static constexpr size_type kTlsLocalExecOffset = 0;
  static constexpr bool kTlsNegative = false;
  static constexpr bool kTpSelfPointer = false;
  static constexpr size_type kTlsRelativeBias = 0;
};

// AArch64 puts TLS above TP after a two-word reserved area.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::kAarch64> : public TlsTraits<Elf, ElfMachine::kNone> {
  using typename TlsTraits<Elf, ElfMachine::kNone>::size_type;

  static constexpr size_type kTlsLocalExecOffset = 2 * sizeof(size_type);
};

// ARM (AArch32) is just the same.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::kArm> : public TlsTraits<Elf, ElfMachine::kAarch64> {};

// RISC-V puts TLS above TP with no offset, as shown in the exemplar.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::kRiscv> : public TlsTraits<Elf, ElfMachine::kNone> {
  using typename TlsTraits<Elf, ElfMachine::kNone>::size_type;

  static constexpr size_type kTlsRelativeBias = 0x800;
};

// x86 puts TLS below TP and requires *$tp = $tp.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::k386> : public TlsTraits<Elf, ElfMachine::kNone> {
  static constexpr bool kTlsNegative = true;
  static constexpr bool kTpSelfPointer = true;
};

// x86-64 uses 64-bit GOT entries even for ILP32.
template <ElfApi Elf>
struct TlsTraitsImpl<Elf, ElfMachine::kX86_64> : public TlsTraits<Elf, ElfMachine::k386> {
  using GotAddr = Elf64<Elf::kData>::Addr;
};

// This should list all the fully-defined specializations except for kNone.
// Note that some generic tests may instantiate a combination of Elf layout
// class and ElfMachine that never actually go together (such as big-endian x86
// or 64-bit i386); this is generally harmless, but does require that whenever
// defining new specializations above, they either be partial specializations
// across layouts or an exhaustive set of full specializations for each layout.
template <template <ElfMachine...> class Template>
using AllSupportedMachines = Template<  //
    ElfMachine::kAarch64, ElfMachine::kArm, ElfMachine::kX86_64, ElfMachine::k386,
    ElfMachine::kRiscv>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_MACHINE_H_
