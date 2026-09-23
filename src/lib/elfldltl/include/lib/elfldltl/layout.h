// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LAYOUT_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LAYOUT_H_

#include <array>
#include <cstddef>
#include <limits>
#include <optional>
#include <string>
#include <string_view>

#include "constants.h"
#include "field.h"
#include "internal/const-string.h"

namespace elfldltl {

// Forward declaration; see <lib/elfldltl/abi-ptr.h>.
struct LocalAbiTraits;

// These types are parameterized by class (32-bit vs 64-bit) and data (byte
// order).  The traditional ELF names Byte, Half, Word, Xword, and Addr are
// used for accessor types that respect the byte order and class.  Note that
// redundant traditional names such as Offset are not used; Addr is used for
// all "address-sized" fields, whether they are offsets, addresses, or sizes.
//
// The Elf<Class, Data> template, abbreviated Elf32<Data> or Elf64<Data>, is
// the intended way to refer to all these types.  The Layout* base types are
// just doing some implementation sharing between template instantiations.
//
// When working with actual values rather than encoded ELF metadata formats,
// the standard uintNN_t types should be used.  The Elf::size_type type is an
// alias for the address-sized unsigned integer type, i.e. the host-side native
// type corresponding to Elf::Addr (which might be a byte-swapping type).
//
// The type and field names for struct types use the traditional terse ELF
// names, but without the traditional prefixes or capitalization.  Each field
// is a byte-order-respecting accessor for the natural underlying type or an
// enum class with the natural underlying type, and has a simple lowercase name
// with no prefix or suffix.  For compound fields, specific accessors are also
// provided to do the bit-field extraction.

template <typename T, typename... AnyOfThese>
inline constexpr bool kIsAnyOf = (std::is_same_v<T, AnyOfThese> || ...);

template <template <typename> typename Template>
struct IsTemplateHelper {
  template <typename T>
  static constexpr bool kIsTemplate = false;

  template <typename T>
  static constexpr bool kIsTemplate<Template<T>> = true;
};

template <typename T, template <typename> typename... AnyOfThese>
inline constexpr bool kIsAnyOfTemplate =
    (IsTemplateHelper<AnyOfThese>::template kIsTemplate<T> || ...);

// The basic types and some structure layouts are identical across bit width
// (ElfClass).  This base class handles differences in byte order (ElfData).
template <ElfData Data>
struct LayoutBase {
  static constexpr ElfData kData = Data;
  static constexpr bool kSwap = Data != ElfData::kNative;

  template <typename T>
  struct UnsignedType {
    using type = UnsignedField<T, kSwap>;
    static_assert(sizeof(type) == sizeof(T));
    static_assert(alignof(type) == alignof(T));
  };

  template <typename T>
  using Unsigned = UnsignedType<T>::type;

  using Byte = Unsigned<uint8_t>;
  using Half = Unsigned<uint16_t>;
  using Word = Unsigned<uint32_t>;
  using Xword = Unsigned<uint64_t>;

  struct Nhdr {
    static constexpr uint32_t Align(uint32_t size) {
      return (size + kAlign - 1) & -uint32_t{kAlign};
    }

    constexpr uint32_t name_offset() const { return sizeof(*this); }

    constexpr uint32_t desc_offset() const { return name_offset() + Align(namesz); }

    constexpr uint32_t size_bytes() const { return desc_offset() + Align(descsz); }

    static constexpr uint32_t kAlign = 4;

    Word namesz;
    Word descsz;
    Word type;
  };
  static_assert(sizeof(Nhdr) == 12);
};

// A base class for the different phdr layouts, ensuring that all of the
// Elf<...>::Phdr definitions below use the same Flags type.
struct PhdrBase {
  // These are individual bits OR'd together.
  enum Flags : uint32_t {
    kExecute = 1 << 0,
    kWrite = 1 << 1,
    kRead = 1 << 2,
  };
};

// A base class for the shdr layout, ensuring that all of the Elf<...>::Shdr
// definitions below use the same Flags type.
struct ShdrBase {
  // These are individual bits OR'd together.
  enum Flags : uint32_t {
    kWrite = 1u << 0,
    kAlloc = 1u << 1,
    kExecinstr = 1u << 2,
    kMerge = 1u << 4,
    kStrings = 1u << 5,
    kInfoLink = 1u << 6,
    kLinkOrder = 1u << 7,
    kOsNonconforming = 1u << 8,
    kGroup = 1u << 9,
    kTls = 1u << 10,
    kCompressed = 1u << 11,
    kOrdered = 1u << 30,
    kExclude = 1u << 31,
  };
};

// A base class for the sym layout, allowing the Elf<...>::Sym definitions
// below to share these methods.
template <class Sym>
struct SymBase {
  constexpr ElfSymBind bind() const {
    return static_cast<ElfSymBind>(static_cast<const Sym*>(this)->info() >> 4);
  }

  constexpr ElfSymType type() const {
    return static_cast<ElfSymType>(static_cast<const Sym*>(this)->info() & 0xf);
  }

  static constexpr uint8_t MakeInfo(ElfSymBind bind, ElfSymType type) {
    return static_cast<uint8_t>((static_cast<uint8_t>(bind) << 4) |
                                (static_cast<uint8_t>(type) << 0));
  }

  constexpr ElfSymVisibility visibility() const {
    return static_cast<ElfSymVisibility>(static_cast<const Sym*>(this)->other() & 0x3);
  }

  // Returns true if this symbol as the referent of a dynamic relocation will
  // always be resolved just to itself in the referring module.  This assumes
  // that STV_PROTECTED does not need to be resolved to ET_EXEC PLT or COPY
  // reloc sites.  Note also that the null symbol with index zero always has
  // all zero fields and thus STB_LOCAL binding, so this returns true for it.
  // Relocations using symbol zero are implicitly resolved as module-relative
  // since its st_value is also zero.
  constexpr bool runtime_local() const {
    return bind() == ElfSymBind::kLocal || visibility() > ElfSymVisibility::kDefault;
  }

  // True if the symbol is visible outside the module for dynamic linking.
  constexpr bool is_public() const {
    if (bind() == ElfSymBind::kLocal) {
      return false;
    }
    switch (visibility()) {
      case ElfSymVisibility::kDefault:
      case ElfSymVisibility::kProtected:
        break;

        // These are supposed to cause the static linker to convert the symbol
        // to STB_LOCAL, but historically there have been linker bugs.
      case ElfSymVisibility::kInternal:
      case ElfSymVisibility::kHidden:
        return false;
    }
    return true;
  }

  // True if the symbol's value field is a vaddr (after load bias).
  constexpr bool has_vaddr() const {
    const Sym& sym = *static_cast<const Sym*>(this);
    // TLS symbols don't have a vaddr; their value is a PT_TLS segment offset.
    // Most undefined symbols don't have a vaddr; their value is just zero.
    // But PLT entries are undefined symbols with their own vaddr.
    return sym.type() != ElfSymType::kTls && (sym.shndx != 0 || sym.value != 0);
  }
};

// Some header layouts vary by ElfClass, i.e. address size used in ELF
// metadata.  This is partially specialized by class below.
template <ElfClass Class, ElfData Data>
struct Layout;

// Layout details specific to 32-bit ELF.
template <ElfData Data>
struct Layout<ElfClass::k32, Data> : public LayoutBase<Data> {
  static constexpr ElfClass kClass = ElfClass::k32;

  using LayoutBase<Data>::kData;
  using LayoutBase<Data>::kSwap;
  using typename LayoutBase<Data>::Byte;
  using typename LayoutBase<Data>::Half;
  using typename LayoutBase<Data>::Word;

  using Addr = LayoutBase<Data>::template Unsigned<uint32_t>;

  struct Phdr : public PhdrBase {
    EnumField<ElfPhdrType, kSwap> type;
    Addr offset;
    Addr vaddr;
    Addr paddr;
    Addr filesz;
    Addr memsz;
    Word flags;
    Addr align;
  };

  struct Sym : public SymBase<Sym> {
    Word name;
    Addr value;
    Addr size;
    Byte info;
    Byte other;
    Half shndx;
  };

  static constexpr unsigned int kRelTypeBits = 8;
};

// Layout details specific to 64-bit ELF.
template <ElfData Data>
struct Layout<ElfClass::k64, Data> : public LayoutBase<Data> {
  static constexpr ElfClass kClass = ElfClass::k64;

  using LayoutBase<Data>::kData;
  using LayoutBase<Data>::kSwap;
  using typename LayoutBase<Data>::Byte;
  using typename LayoutBase<Data>::Half;
  using typename LayoutBase<Data>::Word;

  using Addr = LayoutBase<Data>::template Unsigned<uint64_t>;

  struct Phdr : public PhdrBase {
    EnumField<ElfPhdrType, kSwap> type;
    Word flags;
    Addr offset;
    Addr vaddr;
    Addr paddr;
    Addr filesz;
    Addr memsz;
    Addr align;
  };

  struct Sym : public SymBase<Sym> {
    Word name;
    Byte info;
    Byte other;
    Half shndx;
    Addr value;
    Addr size;
  };

  static constexpr unsigned int kRelTypeBits = 32;
};

// Forward declarations (see note.h).
struct ElfNote;
template <ElfData Data>
class ElfNoteSegment;

// Forward declaration (see tls-layout.h).
template <class Elf>
class TlsLayout;

template <typename Addr>
inline constexpr auto kAddrBits = std::numeric_limits<typename Addr::value_type>::digits;

// The various ELF data structure layouts differ by class (32-bit vs 64-bit).
// But many use the same layout with certain fields being either 32 or 64 bits.
// The layouts that actually differ in field order and the like are defined by
// the Layout base class; the common-by-analogy layouts are defined here.
template <ElfClass Class = ElfClass::kNative, ElfData Data = ElfData::kNative>
struct Elf : private Layout<Class, Data> {
  static constexpr ElfClass kClass = Class;
  static constexpr ElfData kData = Data;

  using Layout<Class, Data>::kSwap;
  using typename Layout<Class, Data>::Byte;
  using typename Layout<Class, Data>::Half;
  using typename Layout<Class, Data>::Word;
  using typename Layout<Class, Data>::Xword;
  using typename Layout<Class, Data>::Addr;

  using size_type = Addr::value_type;

  using Addend = Addr::Signed;

  static constexpr auto kAddressBits = kAddrBits<Addr>;

  using typename Layout<Class, Data>::Nhdr;

  using Note = ElfNote;
  using NoteSegment = ElfNoteSegment<kData>;

  struct Ehdr {
    using ElfLayout = Elf;

    constexpr bool Valid() const {
      return magic == kMagic &&                        // It's ELF at all,
             elfclass == Class && elfdata == Data &&   // of the right sort,
             ident_version == ElfVersion::kCurrent &&  // and passes basic
             version == ElfVersion::kCurrent &&        // sanity checks of
             ehsize == sizeof(Ehdr);                   // various sorts.
    }

    // This is the verbose version that uses the Diagnostics template API (see
    // diagnostics.h) to report why it returns false when it does.
    template <class Diagnostics>
    constexpr bool Valid(Diagnostics& diagnostics) const {
      using namespace std::literals::string_view_literals;

      // The diagnostics object might tell us to keep going after each error.
      bool valid = true;
      auto check = [&](bool ok, auto&& error) -> bool {
        if (ok) {
          return true;
        }
        valid = false;
        return diagnostics.FormatError(std::forward<decltype(error)>(error));
      };

      return check(magic == kMagic, "not an ELF file"sv) &&
             check(elfclass == Class, "wrong ELF class (bit-width)"sv) &&
             check(elfdata == Data, "wrong byte order"sv) &&
             check(ident_version == ElfVersion::kCurrent, "wrong EI_VERSION value"sv) &&
             // The remaining checks rely on correct class and byte order to
             // decode the values meaningfully, so reporting mismatches is
             // redundant after mismatches in the single-byte values.
             valid && check(version == ElfVersion::kCurrent, "wrong e_version value"sv) &&
             check(ehsize == sizeof(Ehdr), "wrong e_ehsize value"sv) && valid;
    }

    constexpr bool Loadable(std::optional<ElfMachine> target = ElfMachine::kNative) const {
      return Valid() && type == ElfType::kDyn && (!target || machine == target);
    }

    // This is the verbose version that uses the Diagnostics template API (see
    // diagnostics.h) to report why it returns false when it does.
    template <class Diagnostics>
    constexpr bool Loadable(Diagnostics& diagnostics,
                            std::optional<ElfMachine> target = ElfMachine::kNative) const {
      using namespace std::literals::string_view_literals;

      if (!Valid(diagnostics)) [[unlikely]] {
        return false;
      }

      if (target && machine != target) [[unlikely]] {
        diagnostics.FormatError("wrong e_machine for architecture"sv);
        return false;
      }

      switch (type()) {
        case ElfType::kDyn:
          [[likely]];
          break;
        case ElfType::kExec:
          diagnostics.FormatError(
              "loading ET_EXEC files is not supported, only ET_DYN files;"
              " be sure to compile and link as PIE (-fPIE, -pie)"sv);
          return false;
        case ElfType::kRel:
          diagnostics.FormatError("ET_REL files cannot be loaded"sv);
          return false;
        case ElfType::kCore:
          diagnostics.FormatError("ET_CORE files cannot be loaded"sv);
          return false;
        default:
          diagnostics.FormatError("unrecognized e_type value"sv, static_cast<uint32_t>(type()));
          return false;
      }

      return true;
    }

    static constexpr Word kMagic{std::array{'\x7f', 'E', 'L', 'F'}};

    // phnum has this value to indicate the real number of phdrs is too large
    // to fit and is instead stored in shdr[0].info.
    static inline const Half kPnXnum{0xffff};

    // These together make up the traditional unsigned char e_ident[16].
    Word magic;
    ElfClass elfclass;
    ElfData elfdata;
    ElfVersion ident_version;
    Byte osabi;
    Byte abiversion;
    Byte ident_pad[7];

    EnumField<ElfType, kSwap> type;
    EnumField<ElfMachine, kSwap> machine;
    EnumField<ElfVersion, kSwap, uint32_t> version;
    Addr entry;
    Addr phoff;
    Addr shoff;
    Word flags;
    Half ehsize;
    Half phentsize;
    Half phnum;
    Half shentsize;
    Half shnum;
    Half shstrndx;
  };

  using typename Layout<Class, Data>::Phdr;

  // This is not really used at runtime except for the kPnXnum protocol.
  // But it's useful to have all the values handy for diagnostic tools.
  struct Shdr : public ShdrBase {
    Word name;
    EnumField<ElfShdrType, kSwap> type;
    Addr flags;
    Addr addr;
    Addr offset;
    Addr size;
    Word link;
    Word info;
    Addr addralign;
    Addr entsize;
  };

  struct Dyn {
    EnumField<ElfDynTag, kSwap, size_type> tag;

    // Traditionally this was a union d_un of d_val and d_ptr, but both with
    // types that are just an address-sized unsigned integer.  Sometimes the
    // value is a "pointer" (relative to load bias) to some data structure.
    // Sometimes it's a byte size.  Sometimes it's an enum constant.
    Addr val;
  };

  using typename Layout<Class, Data>::Sym;

  // These two classes are copied so that these types can be both easily
  // aggregate initialized and designated initialized. We can't have both
  // if we opt to use inheritance to save some code duplication in these
  // two classes. Note, Phdr and Sym types use inheritance in this way
  // and the syntax to aggregate initialize them is ugly {{}, fields...}.
  // In practice, those types are never aggregate initialized because their
  // layout differs between elf classes. The Rel types do not so we wish
  // for them to be easily aggregate initializable. Likewise, we want
  // these types to follow Phdr and Sym and be designated initializable
  // even if this pattern is unlikely to be used on these types.

  struct Rel {
    constexpr uint32_t symndx() const { return static_cast<uint32_t>(info() >> kSymndxShift); }
    constexpr uint32_t type() const { return info() & kTypeMask; }

    static constexpr auto kSymndxShift = Layout<Class, Data>::kRelTypeBits;
    static constexpr auto kTypeMask = (size_type{1} << kSymndxShift) - 1;

    static constexpr Addr MakeInfo(Addr sym_name, uint32_t type) {
      return (sym_name << kSymndxShift) | (static_cast<Addr>(type) & kTypeMask);
    }

    Addr offset;
    Addr info;
  };

  struct Rela {
    constexpr uint32_t symndx() const { return static_cast<uint32_t>(info() >> kSymndxShift); }
    constexpr uint32_t type() const { return info() & kTypeMask; }

    static constexpr auto kSymndxShift = Layout<Class, Data>::kRelTypeBits;
    static constexpr auto kTypeMask = (size_type{1} << kSymndxShift) - 1;

    static constexpr Addr MakeInfo(Addr sym_name, uint32_t type) {
      return (sym_name << kSymndxShift) | (static_cast<Addr>(type) & kTypeMask);
    }

    Addr offset;
    Addr info;
    Addend addend;
  };

  // Usually GOT entries are address-sized, so 32 bits for ILP32.
  // But on x86-64 ILP32, they are instead register-sized (64 bits).
  template <ElfMachine Machine = ElfMachine::kNative>
  using GotEntry = std::conditional_t<Machine == ElfMachine::kX86_64, Xword, Addr>;

  // When the compiler generates a call to __tls_get_addr, the linker
  // generates two corresponding dynamic relocation entries applying to
  // adjacent GOT slots that form a pair describing what module and symbol
  // resolved the reference at dynamic link time.  The first slot holds the
  // module ID, a 1-origin index.  The second slot holds the offset from
  // that module's PT_TLS segment.
  template <typename GotEntry>
  struct TlsGetAddrGotLayout {
    GotEntry tls_modid;  // R_*_DTPMOD* et al relocations set this.
    GotEntry offset;     // R_*_DTPOFF* et al relocations set this.
  };
  template <ElfMachine Machine = ElfMachine::kNative>
  using TlsGetAddrGot = TlsGetAddrGotLayout<GotEntry<Machine>>;

  // When the compiler generates a TLSDESC callback, the linker generates a
  // single corresponding dynamic relocation entry that applies to a pair of
  // adjacent GOT slots.  In DT_REL format, the addend is stored in the second
  // slot.  The first slot holds a function pointer installed by the dynamic
  // linker.  The compiler generates code to call this function pointer using a
  // bespoke calling convention specified in each psABI; it takes a single
  // argument of this address in the GOT.  The second slot is filled by the
  // dynamic linker with whatever value is of use to the function it installs
  // such that it returns the thread pointer offset of the per-thread address
  // of the relocation's symbol plus addend.  (For static TLS, that offset will
  // be the same in every thread.  For dynamic TLS, it will be the difference
  // of unrelated pointers that recovers an uncorrelated per-thread address.)
  template <typename GotEntry>
  struct TlsDescGotLayout {
    GotEntry function;
    GotEntry value;
  };
  template <ElfMachine Machine = ElfMachine::kNative>
  using TlsDescGot = TlsDescGotLayout<GotEntry<Machine>>;

  // These are declared in svr4-abi.h rather than there.  These are not
  // formally parts of the ELF format, but rather de facto standard ABI types
  // from the original SVR4 implementation that introduced ELF that have been
  // kept compatible in other implementations historically.

  template <class AbiTraits = LocalAbiTraits>
  struct LinkMap;

  template <class AbiTraits = LocalAbiTraits>
  struct RDebug;

  // This is true for any T that's an layout.h or field.h type, i.e. guaranteed
  // to use the right format and byte order for the given ELF Class and Data.
  // Note that all <lib/elfldltl/field.h> types yield true even if they aren't
  // sized or byte orders used in this Elf instantiation: they are still types
  // whose exact intended target format is both well-known and represented for
  // byte-by-byte copying directly in T.  Conversely, note neither LinkMap,
  // RDebug, nor any other type that indirectly uses AbiPtr, is considered a
  // "layout" type; these types are specialized by an AbiTraits parameter that
  // determines their actual memory format.  All non-layout types (aside from
  // single-byte integers and the like) require specialized transcription when
  // copying between address spaces or pointer formats.  This is defined
  // primarily for the benefit of <lib/ld/remote-abi-transcriber.h>, which see.
  template <typename T>
  static constexpr bool kIsLayout =
      kIsAnyOf<T, Ehdr, Shdr, Nhdr, Phdr, Dyn, Sym, Rel, Rela, TlsLayout<Elf>> ||
      kIsAnyOfTemplate<T, TlsGetAddrGotLayout, TlsDescGotLayout>;

  // These are usable as std::string_view but also have a .c_str() method with
  // std::string-compatible semantics.
  static constexpr internal::ConstString kName{[]() {
    using namespace std::literals;
    const std::string data{ElfDataName(kData)};
    return "elf"s + internal::ToString(kAddressBits) + data;
  }};
  static constexpr internal::ConstString kUpperName{[]() {
    using namespace std::literals;
    const std::string data{ElfDataName(kData, true)};
    return "ELF"s + internal::ToString(kAddressBits) + data;
  }};

  // This can be used to assemble a file name based on kName and optionally
  // machine.  The string looks like "<prefix>elf64le-<cpu><suffix>" or
  // "<prefix>elf32le><suffix>" (for ElfMachine::kNone), etc.
  template <const std::string_view& kPrefix, ElfMachine Machine, const std::string_view& kSuffix>
  static constexpr internal::ConstString kFilename{[]() {
    using namespace std::literals;
    std::string name{kPrefix};
    name += kName;
    if (Machine != ElfMachine::kNone) {
      name += '-';
      name += ElfMachineFileName(Machine, kClass);
    }
    name += kSuffix;
    return name;
  }};
};

template <ElfData Data = ElfData::kNative>
using Elf32 = Elf<ElfClass::k32, Data>;

template <ElfData Data = ElfData::kNative>
using Elf64 = Elf<ElfClass::k64, Data>;

// This instantiates Template with Elf64<> and Elf32<> as parameters.
template <template <class...> typename Template>
using AllNativeFormats = Template<Elf64<>, Elf32<>>;

// This instantiates Template with each Elf variant as a parameter.
template <template <class...> typename Template>
using AllFormats = Template<Elf64<ElfData::k2Lsb>, Elf32<ElfData::k2Lsb>,  //
                            Elf64<ElfData::k2Msb>, Elf32<ElfData::k2Msb>>;

template <typename T>
struct IsAnyElf {
  template <class... Elf>
  using type = std::bool_constant<(std::same_as<T, Elf> || ...)>;
};

template <typename T>
concept ElfApi = static_cast<bool>(AllFormats<IsAnyElf<T>::template type>{});

template <typename T>
struct IsAnyLayout {
  template <ElfApi... Elf>
  using type = std::bool_constant<(Elf::template kIsLayout<T> || ...)>;
};

// Like Elf::kIsLayout, but across any known format.
template <typename T>
static constexpr bool kIsLayout = AllFormats<IsAnyLayout<T>::template type>{};

template <ElfClass Class, ElfData Data>
template <typename T, bool kSwap>
constexpr bool Elf<Class, Data>::kIsLayout<UnsignedField<T, kSwap>> = true;

template <ElfClass Class, ElfData Data>
template <typename T, bool kSwap>
constexpr bool Elf<Class, Data>::kIsLayout<SignedField<T, kSwap>> = true;

template <ElfClass Class, ElfData Data>
template <typename T, bool kSwap, typename U>
constexpr bool Elf<Class, Data>::kIsLayout<EnumField<T, kSwap, U>> = true;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_LAYOUT_H_
