// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_H_

// This file provides template APIs that do the central orchestration of
// dynamic linking: resolving and applying relocations.

#include <lib/fit/result.h>

#include <concepts>

#include "diagnostics.h"
#include "machine.h"
#include "memory.h"
#include "relocation.h"

namespace elfldltl {

// Apply simple fixups as directed by Elf::RelocationInfo, given the load bias:
// the difference between runtime addresses and addresses that appear in the
// relocation records.  This calls memory.Store(reloc_address, runtime_address)
// or memory.StoreAdd(reloc_address, bias) to store the adjusted values.
// Returns false iff any calls into the Memory object returned false.
template <class Diagnostics, class RelocInfo>
constexpr bool RelocateRelative(
    Diagnostics& diag,
    MemoryWriter<typename RelocInfo::size_type, typename RelocInfo::Addr> auto& memory,
    const RelocInfo& info, typename RelocInfo::size_type bias) {
  using Addr = RelocInfo::Addr;
  using size_type = RelocInfo::size_type;

  struct Visitor {
    constexpr bool CheckStore(bool b, size_type addr) {
      if (!b) [[unlikely]] {
        return diag_.FormatError("invalid address in RELATIVE relocation", FileAddress{addr});
      }
      return true;
    }

    // RELA entry with separate addend.
    constexpr bool operator()(const RelocInfo::Rela& reloc) {
      auto addr = bias_ + reloc.addend();
      return CheckStore(memory_.template Store<Addr>(reloc.offset, addr), reloc.offset);
    }

    // REL or RELR entry with addend in place.
    constexpr bool operator()(size_type addr) {
      return CheckStore(memory_.template StoreAdd<Addr>(addr, bias_), addr);
    }

    decltype(memory) memory_;
    Diagnostics& diag_;
    size_type bias_;
  };

  return info.VisitRelative(Visitor{memory, diag, bias});
}

// Symbolic relocation for STT_TLS symbols requires the symbolic resolution
// engine meet different invariants depending on the specific relocation type.
enum class RelocateTls {
  kNone,     // Not TLS.
  kDynamic,  // Dynamic TLS reloc: the defining module will have a TLS segment.
  kStatic,   // Static TLS reloc: the defining module needs static TLS layout.
  kDesc,     // TLSDESC reloc: the definition must supply hook and parameter.
};

// This concept defines the contract for types referred to as Definition in the
// RelocateSymbolic API specified here.  See <lib/elfldltl/resolve.h> for some
// examples implementing this contract.
//
// Callback methods that can fail return `fit::result<bool, ...>`.  The error
// value is like the return value from a Diagnostics object, indicating whether
// to continue applying more relocations or to bail out immediately.
template <class Definition, class Elf, class Diagnostics, ElfMachine Machine>
concept RelocateSymbolicDefinitionApi =
    ElfApi<Elf> &&
    requires(const Definition& defn) {
      // Returns true iff the symbol was resolved as an undefined weak
      // reference.
      { defn.undefined_weak() } -> std::convertible_to<bool>;

      // Returns the load bias for symbol addresses in the defining module.
      { defn.bias() } -> std::convertible_to<typename Elf::size_type>;

      // Returns the defining symbol table entry.
      { defn.symbol() } -> std::convertible_to<const typename Elf::Sym&>;

      // Returns the TLS module ID number for the defining module.
      { defn.tls_module_id() } -> std::convertible_to<typename Elf::size_type>;

      // Returns the static TLS layout bias for the defining module.
      { defn.static_tls_bias() } -> std::convertible_to<typename Elf::size_type>;
    } &&  // If the one-argument tls_desc_undefined_weak() is provided, it will
          // always be used.  If not, the no-argument tls_desc_undefined_weak()
          // must be provided instead.
    (
        requires(const Definition& defn, Elf::Addend addend) {
          // This is only called if undefined_weak() returned true first.
          // Returns the GOT contents for a TLSDESC resolution that was an
          // undefined weak.  This overload can use the relocation addend in
          // choosing the particular TLSDESC implementation, but this requires
          // an extra load in the DT_REL case.  The return value will be used
          // as is.
          {
            defn.tls_desc_undefined_weak(addend)
          } -> std::convertible_to<typename Elf::template TlsDescGot<Machine>>;
        } ||
        requires(const Definition& defn) {
          // This is only called if undefined_weak() returned true first.
          // Returns the GOT contents for a TLSDESC resolution that was an
          // undefined weak symbol.  This overload does not require the addend
          // up front.  Instead, the TlsDescGot::value field will have the
          // addend applied implicitly.
          {
            defn.tls_desc_undefined_weak()
          } -> std::convertible_to<typename Elf::template TlsDescGot<Machine>>;
        }) &&  // If the two-argument tls_desc() is provided, it will always be
               // used.  If not, the one-argument tls_desc() must be provided
               // instead.
    (
        requires(const Definition& defn, Elf::Addend addend, Diagnostics& diag) {
          // Returns the GOT contents for a TLSDESC resolution, which can fail.
          // If this overload is present, then it is always used and the second
          // overload below need not be defined.  This overload can use the
          // relocation addend in choosing the TLSDESC implementation, but this
          // requires an extra load in the DT_REL case.  The return value will
          // be used as is.
          {
            defn.tls_desc(diag, addend)
          } -> std::same_as<fit::result<bool, typename Elf::template TlsDescGot<Machine>>>;
        } ||
        requires(const Definition& defn, Diagnostics& diag) {
          // Returns the GOT contents for a TLSDESC resolution, which can fail.
          // This overload does not require the addend up front.  Instead, the
          // TlsDescGot::value field will have the addend applied implicitly.
          {
            defn.tls_desc(diag)
          } -> std::same_as<fit::result<bool, typename Elf::template TlsDescGot<Machine>>>;
        });

// This is a helper concept, describing the fit::result<bool, ...> return value
// for a RelocateSymbolicResolverApi<Elf> callable.
template <typename T, class Elf, class Diagnostics, ElfMachine Machine>
concept RelocateSymbolicResolverResultApi =
    std::same_as<T, fit::result<bool, typename T::value_type>> &&
    RelocateSymbolicDefinitionApi<typename T::value_type, Elf, Diagnostics, Machine>;

// This describes the callback passed to RelocateSymbolic to do the actual
// symbol resolution.  It returns fit::result<bool, Definition> where the
// error_value treatment is as described for Definition methods above.
template <typename T, class Elf, class Diagnostics, ElfMachine Machine>
concept RelocateSymbolicResolverApi =
    ElfApi<Elf> && std::invocable<T, const typename Elf::Sym&, RelocateTls> &&
    RelocateSymbolicResolverResultApi<
        std::invoke_result_t<T, const typename Elf::Sym&, RelocateTls>,  //
        Elf, Diagnostics, Machine>;

// Apply symbolic relocations as directed by Elf::RelocationInfo, referring to
// Elf::SymbolInfo as adjusted by the load bias (as used in RelocateRelative).
// The callback function and methods on its returned Definition here use
// `fit::result<bool, ...>` for cases where failure to deliver a result is an
// option.  The callback function is:
//
//  * fit::result<bool, Definition> resolve(const Sym&, RelocateTls)
//
// where Definition is some type defined by the caller that supports methods:
//
template <ElfMachine Machine = ElfMachine::kNative, class DiagnosticsType, class RelocInfo,
          class SymbolInfo,
          RelocateSymbolicResolverApi<typename RelocInfo::Elf, DiagnosticsType, Machine> Resolve>
constexpr bool RelocateSymbolic(
    MemoryApi<typename RelocInfo::size_type, typename RelocInfo::Elf::Addr,
              typename RelocInfo::Elf::Addend> auto& memory,
    DiagnosticsType& diagnostics, const RelocInfo& reloc_info, const SymbolInfo& symbol_info,
    typename RelocInfo::size_type bias, Resolve&& resolve) {
  using namespace std::literals;

  using Elf = RelocInfo::Elf;
  using Addr = Elf::Addr;
  using Addend = Elf::Addend;
  using size_type = Elf::size_type;
  using Rel = Elf::Rel;
  using Rela = Elf::Rela;
  using TlsDescGot = Elf::template TlsDescGot<Machine>;

  static_assert(std::same_as<typename SymbolInfo::Addr, Addr>,
                "incompatible RelocInfo and SymbolInfo types passed to elfldltl::RelocateSymbolic");
  using Sym = SymbolInfo::Sym;

  static_assert(std::invocable<Resolve, const Sym&, RelocateTls>,
                "elfldltl::RelocateSymbolic requires resolve(const Sym&, RelocateTls) callback");

  using Traits = RelocationTraits<Machine>;
  using Tls = TlsTraits<Elf, Machine>;
  using Type = Traits::Type;

  // Apply either a REL or RELA reloc resolved to a value, ignoring the addend.
  auto apply_no_addend = [&](const auto& reloc, size_type value) -> bool {
    return memory.template Store<Addr>(reloc.offset, value);
  };

  // Apply either a REL or RELA reloc resolved to a value, using the addend.
  auto apply_with_addend = [&](const auto& reloc, size_type value) -> bool {
    if constexpr (std::same_as<decltype(reloc), const Rel&>) {
      return memory.template StoreAdd<Addr>(reloc.offset, value);
    } else {
      static_assert(std::same_as<decltype(reloc), const Rela&>);
      return memory.template Store<Addr>(reloc.offset, value + reloc.addend());
    }
  };

  // Resolve a symbolic relocation and call apply(reloc, defn) to apply it to
  // the resolved Definition object (as `const&`), if successful.
  auto relocate_symbolic = [&](const auto& reloc, auto&& apply,
                               RelocateTls tls = RelocateTls::kNone) -> bool {
    const uint32_t symndx = reloc.symndx();
    decltype(auto) symtab = symbol_info.symtab();
    if (symndx >= symtab.size()) [[unlikely]] {
      return diagnostics.FormatError("relocation entry symbol table index out of bounds"sv);
    }
    const Sym& sym = symtab[symndx];
    // Symbol index zero is mostly treated like anything else because the null
    // symbol at index zero's all-zero fields map to an STB_LOCAL symbol with
    // st_value of zero, which does the right thing.  However, it's also an
    // STT_NOTYPE symbol but still does the right thing for a TLS relocation.
    if (symndx != 0) {
      const bool is_tls = tls != RelocateTls::kNone;
      if ((sym.type() == ElfSymType::kTls) != is_tls) [[unlikely]] {
        return diagnostics.FormatError(  //
            is_tls ? "TLS relocation entry with non-STT_TLS symbol: "sv
                   : "non-TLS relocation entry with STT_TLS symbol: "sv,
            symbol_info.string(sym.name));
      }
    }
    auto defn = resolve(sym, tls);
    return defn.is_error() ? defn.error_value() : apply(reloc, *defn);
  };

  // The Definition for a non-TLS reloc is applied by passing the biased symbol
  // value to the low-level apply_{no,with}_addend function.
  constexpr auto nontls = [](auto&& apply) {
    return [apply](const auto& reloc, const auto& defn) {
      if (defn.undefined_weak()) {
        return apply(reloc, 0);
      }
      return apply(reloc, defn.symbol().value() + defn.bias());
    };
  };

  // Static TLS relocs resolve to an offset from the thread pointer.  Each
  // module capable of resolving definitions for static TLS relocs will have
  // had a static TLS layout assigned.  Consistent with the glibc behavior, an
  // undefined weak symbol results in not applying the relocation at all, which
  // results in an offset from the thread pointer that's either zero or is the
  // reloc's nonzero addend in the DT_REL (vs DT_RELA) case.
  auto tls_absolute = [apply_with_addend](const auto& reloc, const auto& defn) {
    return defn.undefined_weak() ||
           apply_with_addend(reloc, defn.symbol().value() + defn.static_tls_bias());
  };

  // TLSMOD relocs resolve to a module ID (index), not an address value.  This
  // is stored in a GOT slot to be passed to __tls_get_addr.  Consistent with
  // the glibc behavior, an undefined weak symbol results in not applying the
  // relocation at all, leaving the slot to yield module ID zero.  The glibc
  // __tls_get_addr will not handle that well--it's really not expected to be
  // called at all when the symbol is an undefined weak, so nothing should
  // really care what's in the GOT slots.
  auto tls_module = [apply_no_addend](const auto& reloc, const auto& defn) {
    return defn.undefined_weak() || apply_no_addend(reloc, defn.tls_module_id());
  };

  // Dynamic TLS relocs resolve to an offset into the defining module's TLS
  // segment, stored in a GOT slot to be passed to __tls_get_addr.  The
  // undefined weak case is as for tls_module (above), see comments there.
  auto tls_relative = [apply_with_addend](const auto& reloc, const auto& defn) {
    return defn.undefined_weak() ||
           apply_with_addend(reloc, defn.symbol().value() - Tls::kTlsRelativeBias);
  };

  // Each TLSDESC reloc acts like two relocs to consecutive GOT slots: first
  // the hook function, which is passed the address of its own GOT slot; then a
  // stored value for the hook to retrieve.  The reloc's addend is applied only
  // to the value (for REL, stored in place in that second slot).  The resolver
  // selects an appropriate hook function for the defining module (static or
  // dynamic), and the encoding of the value is between the resolver and its
  // hook function (except that it must be some sort of byte offset in a TLS
  // block such that applying the addend here makes sense).
  auto tls_desc = [&](const auto& reloc, const auto& defn) -> bool {
    // reloc.offset points to the function slot but indicates filling both
    // slots.  value_reloc will point to the second slot, which is where
    // the addend is used.
    auto value_reloc = reloc;
    value_reloc.offset = reloc.offset + sizeof(size_type);

    // Call either signature of the Definition::tls_desc_undefined_weak method.
    // It cannot fail, so wrap it in an OK result.  The return type is always
    // fit::result<bool, TlsDescGot>, which is what would be deduced.  But
    // using explicit decltype on the call expression makes calls
    // SFINAE-friendly so that std::invocable can be used below.  Otherwise the
    // lambda looks like it can be called with any arguments.
    auto weak_desc = [&defn](auto&&... args)
        -> fit::result<  //
            bool, decltype(defn.tls_desc_undefined_weak(std::forward<decltype(args)>(args)...))> {
      return fit::ok(defn.tls_desc_undefined_weak(std::forward<decltype(args)>(args)...));
    };

    // Call either signature of the Definition::tls_desc method.  It already
    // returns the result type, but the SFINAE dance is required here too.
    auto defn_desc = [&diagnostics, &defn](auto&&... args)
        -> decltype(defn.tls_desc(diagnostics, std::forward<decltype(args)>(args)...)) {
      return defn.tls_desc(diagnostics, std::forward<decltype(args)>(args)...);
    };

    auto apply = [reloc, value_reloc, apply_no_addend, apply_with_addend,
                  &memory](auto&& get_desc) -> bool {
      using Reloc = std::decay_t<decltype(value_reloc)>;
      constexpr bool kAddend = std::invocable<decltype(get_desc), Addend>;

      const auto& apply_value = [&]() -> auto& {
        if constexpr (kAddend) {
          // The callback gets the addend and includes it in the value.
          return apply_no_addend;
        } else {
          // The callback doesn't see the addend, so it's implicitly applied
          // to the value slot.
          return apply_with_addend;
        }
      }();

      auto get_addend = [&]() -> fit::result<bool, Addend> {
        if constexpr (std::same_as<Reloc, Rel>) {
          // The addend must be read out of the memory being relocated.
          auto read = memory.template ReadArray<Addend>(value_reloc.offset, 1);
          if (!read) {
            return fit::error{false};
          }
          return fit::ok(read->front());
        } else {
          static_assert(std::same_as<Reloc, Rela>);
          std::ignore = &memory;
          return fit::ok(value_reloc.addend);
        }
        __builtin_unreachable();
      };

      auto resolve_desc = [&]() -> fit::result<bool, TlsDescGot> {
        if constexpr (kAddend) {
          // Definition::tls_get_desc(Addend) is available, so use it.
          auto addend = get_addend();
          return addend.is_error() ? addend.take_error() : get_desc(*addend);
        } else {
          // Just use Definition::tls_get_desc() without giving it the addend.
          // The addend will be implicitly applied to the value slot.
          return get_desc();
        }
        __builtin_unreachable();
      };

      fit::result<bool, TlsDescGot> desc = resolve_desc();

      return desc.is_error() ? desc.error_value()
                             : (apply_no_addend(reloc, desc->function) &&
                                apply_value(value_reloc, desc->value));
    };

    return defn.undefined_weak() ? apply(weak_desc) : apply(defn_desc);
  };

  // Apply a single relocation record by dispatching on its type.
  auto relocate = [&](const auto& reloc) -> bool {
    const uint32_t type = reloc.type();

    switch (static_cast<Type>(type)) {
      case Type::kNone:
        return diagnostics.FormatWarning("R_*_NONE relocation record encountered"sv);

      case Type::kRelative:
        return diagnostics.FormatWarning("R_*_RELATIVE relocation record not sorted properly"sv) &&
               apply_with_addend(reloc, bias);

      case Type::kAbsolute:
        return relocate_symbolic(reloc, nontls(apply_with_addend));

      case Type::kPlt:
        return relocate_symbolic(reloc, nontls(apply_no_addend));

      case Type::kTlsModule:
        return relocate_symbolic(reloc, tls_module, RelocateTls::kDynamic);

      case Type::kTlsAbsolute:
        return relocate_symbolic(reloc, tls_absolute, RelocateTls::kStatic);

      case Type::kTlsRelative:
        return relocate_symbolic(reloc, tls_relative, RelocateTls::kDynamic);
    }

    // These two are not in the enum because they don't exist on every machine.
    // So they are defined as std::optional<uint32_t> and may be std::nullopt,
    // which will make these tautological comparisons that get eliminated.

    if (type == Traits::kGot) {
      return relocate_symbolic(reloc, nontls(apply_no_addend));
    }

    if (type == Traits::kTlsDesc) {
      return relocate_symbolic(reloc, tls_desc, RelocateTls::kDesc);
    }

    return diagnostics.FormatError("unrecognized relocation type"sv, type);
  };

  return reloc_info.VisitSymbolic(relocate);
}

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LINK_H_
