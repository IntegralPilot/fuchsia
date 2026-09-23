// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PHDR_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PHDR_H_

#include <algorithm>
#include <bit>
#include <cassert>
#include <limits>
#include <optional>
#include <span>
#include <string_view>
#include <type_traits>

#include "constants.h"
#include "diagnostics.h"
#include "internal/no_unique_address.h"
#include "internal/phdr-error.h"
#include "layout.h"
#include "memory.h"

namespace elfldltl {

// Read program headers from an ELF file using the File API (see memory.h),
// given its file header (Ehdr) already read and validated.
//
// This takes a Diagnostics object (see diagnostics.h) and a File object (see
// memory.h) by reference.  It ultimately calls `file.ReadArrayFromFile<Phdr>`
// and returns that value if all goes well.  The Allocator argument is just
// forwarded to File::ReadArrayFromFile; see memory.h for API details.
//
// All the template parameters are normally deduced, including the Elf layout
// to use based on the Ehdr type used for the last argument.
template <class Diagnostics, class Ehdr,
          ReadArrayFromFileAllocator<typename Ehdr::ElfLayout::Phdr, uint32_t> Allocator>
constexpr auto ReadPhdrsFromFile(
    Diagnostics& diagnostics,
    CanReadArrayFromFile<typename Ehdr::ElfLayout::Phdr, Allocator,
                         typename Ehdr::ElfLayout::size_type> auto& file,
    Allocator&& allocator, const Ehdr& ehdr) {
  using namespace std::literals::string_view_literals;

  using Elf = Ehdr::ElfLayout;
  using size_type = Elf::size_type;
  using Phdr = Elf::Phdr;
  using Shdr = Elf::Shdr;

  const size_type phoff = ehdr.phoff;
  auto read_phdrs = [&](size_t phnum) {
    auto result = file.template ReadArrayFromFile<Phdr>(  //
        phoff, std::forward<Allocator>(allocator), phnum);
    if (!result) {
      diagnostics.FormatError("cannot read program headers from ELF file"sv, FileOffset{phoff});
    }
    return result;
  };
  using Result = std::decay_t<decltype(read_phdrs(0).value())>;
  using OptionalResult = std::optional<Result>;
  static_assert(std::is_same_v<OptionalResult, decltype(read_phdrs(0))>);

  // An empty phdr table is not an error, just an empty result.
  if (ehdr.phnum == 0) [[unlikely]] {
    return OptionalResult(Result());
  }

  // Validate the Ehdr fields related to the phdr.

  if (ehdr.phentsize != sizeof(Phdr)) [[unlikely]] {
    diagnostics.FormatError("e_phentsize has unexpected value"sv, ehdr.phentsize());
    return OptionalResult();
  }

  if (phoff < sizeof(Ehdr)) [[unlikely]] {
    diagnostics.FormatError("e_phoff overlaps with ELF file header"sv, FileOffset{phoff});
    return OptionalResult();
  }

  if (phoff % alignof(Phdr) != 0) [[unlikely]] {
    diagnostics.FormatError("e_phoff has insufficient alignment"sv, FileOffset{phoff});
    return OptionalResult();
  }

  // Things look valid, so find the count.  Usually it's directly in the field.
  if (ehdr.phnum != Elf::Ehdr::kPnXnum) [[likely]] {
    return read_phdrs(ehdr.phnum);
  }

  // This is the marker that the count might exceed 16 bits.  In that case,
  // it's instead stored in the special stub section header at index 0.
  // This is the only time the section header table is used at runtime,
  // and there are still no actual sections (index 0 is always a stub).

  if (ehdr.shnum == 0) [[unlikely]] {
    diagnostics.FormatError("PN_XNUM with no section headers"sv);
    return OptionalResult();
  }

  if (ehdr.shentsize != sizeof(Shdr)) [[unlikely]] {
    diagnostics.FormatError("e_shentsize has unexpected value"sv, ehdr.shentsize());
    return OptionalResult();
  }

  const size_type shoff = ehdr.shoff;
  if (shoff < sizeof(Ehdr)) [[unlikely]] {
    diagnostics.FormatError("e_shoff overlaps with ELF file header"sv, FileOffset{shoff});
    return OptionalResult();
  }

  if (auto read_shdr = file.template ReadFromFile<Shdr>(shoff)) [[likely]] {
    const Shdr& shdr = *read_shdr;
    return read_phdrs(shdr.info);
  }

  diagnostics.FormatError("cannot read section header 0 from ELF file"sv, FileOffset{shoff});
  return OptionalResult();
}

// elfldltl::DecodePhdrs does a single-pass decoding of a program
// header table by statically combining multiple "observer" objects.  Various
// observers are defined to collect different subsets of the segment metadata
// needed for different purposes.

// This represents a program header entry that's been matched to a specific
// segment type. These types are used in the arguments to Observe callbacks;
// see below.
template <ElfPhdrType Type>
struct PhdrTypeMatch {};

// This is the base class for Phdr*Observer classes.
// Each Observer subclass should define these two methods:
//  * template <class Diagnostics, typename Phdr>
//    bool Observe(Diagnostics& diagnostics, PhdrTypeMatch<Type> type, const Phdr& val);
//  * template <class Diagnostics>
//    bool Finish(Diagnostics& diagnostics);
// Observe will be called with each entry matching any Type of the Types...
// list. Then Finish will be called at the end of all entries unless processing
// was terminated early for some reason, in which case the observer object is
// usually going to be destroyed without checking its results.  Both return
// false if processing the phdr table should be terminated early.
//
// In practice, a given observer will correspond to a single ElfPhdrType;
// however, the parameter is kept variadic for consistency with other APIs in
// the library.
template <class Observer, ElfPhdrType... Types>
struct PhdrObserver {};

// This decodes a program header table by matching each entry against a list of
// observers.  Each observer should be of a subclass of PhdrObserver that
// indicates the segment type it matches.  If any matching observer returns
// false then this stops processing early and returns false.  Otherwise, each
// observer's Finish method is called, stopping early if one returns false.
template <class Diagnostics, class Phdr, size_t N, class... Observers>
constexpr bool DecodePhdrs(Diagnostics&& diagnostics, std::span<const Phdr, N> phdrs,
                           Observers&&... observers) {
  for (const Phdr& phdr : phdrs) {
    if ((!DecodePhdr(diagnostics, phdr, observers) || ...)) {
      return false;
    }
  }
  return (observers.Finish(diagnostics) && ...);
}

// Match a single program header against a single observer.  If the
// observer matches, its Observe overload for the matching tag is called.
// Returns the value of that call, or true if this observer didn't match.
template <class Diagnostics, class Phdr, class Observer, ElfPhdrType... Type>
constexpr bool DecodePhdr(Diagnostics&& diagnostics, const Phdr& phdr,
                          PhdrObserver<Observer, Type...>& observer) {
  constexpr auto kKnownFlags = Phdr::kRead | Phdr::kWrite | Phdr::kExecute;

  bool ok = true;
  auto check_header = [&](auto error) {
    using Error = decltype(error);

    if ((phdr.flags() & ~kKnownFlags) != 0) {
      if (ok = diagnostics.FormatWarning(Error::kUnknownFlags); !ok) {
        return false;
      }
    }

    // An `align` of 0 signifies no alignment constraints, which in practice
    // means an alignment of 1.
    auto align = phdr.align() > 0 ? phdr.align() : 1;
    if (!std::has_single_bit(align)) {
      if (ok = diagnostics.FormatError(Error::kBadAlignment); !ok) {
        return false;
      }
    }

    // While not a general spec'd constraint, this is the case in practice:
    // either this a explicit requirement or the stronger constraint of
    // `p_offset` and `p_vaddr` being `p_align`-aligned (e.g., zero) is
    // expected to hold.
    if (phdr.offset() % align != phdr.vaddr() % align) {
      if (ok = diagnostics.FormatError(Error::kOffsetNotEquivVaddr); !ok) {
        return false;
      }
    }

    return true;
  };
  auto call_observer = [&](auto type) {
    ok = static_cast<Observer&>(observer).Observe(diagnostics, type, phdr);
    return ok;
  };

  ((phdr.type == Type && check_header(internal::PhdrError<Type>{}) &&
    call_observer(PhdrTypeMatch<Type>{})) ||
   ...);
  return ok;
}

template <ElfApi Elf>
class PhdrNullObserver : public PhdrObserver<PhdrNullObserver<Elf>, ElfPhdrType::kNull> {
 public:
  using Phdr = Elf::Phdr;

  template <class Diagnostics>
  constexpr bool Observe(Diagnostics& diagnostics, PhdrTypeMatch<ElfPhdrType::kNull> type,
                         const Phdr& phdr) {
    return diagnostics.FormatWarning("PT_NULL header encountered");
  }

  template <class Diagnostics>
  constexpr bool Finish(Diagnostics& diagnostics) {
    return true;
  }
};

// A class of observer corresponding to the simpler segment metadata types:
// it merely stores any program header that it sees at the provided reference,
// complaining if it observes more than one segment of the same type.
template <ElfApi Elf, ElfPhdrType Type>
class PhdrSingletonObserver : public PhdrObserver<PhdrSingletonObserver<Elf, Type>, Type> {
 public:
  using Phdr = Elf::Phdr;

  explicit PhdrSingletonObserver(std::optional<Phdr>& phdr) : phdr_(phdr) {}

  template <class Diagnostics>
  constexpr bool Observe(Diagnostics& diagnostics, PhdrTypeMatch<Type> type, const Phdr& phdr) {
    // Warning, since a wrong PHDRS clause in a linker script could cause
    // this and be harmless in practice rather than indicating a linker bug
    // or corrupted data.
    if (phdr_ && !diagnostics.FormatWarning(internal::PhdrError<Type>::kDuplicateHeader)) {
      return false;
    }
    phdr_ = phdr;
    return true;
  }

  template <class Diagnostics>
  constexpr bool Finish(Diagnostics& diagnostics) {
    return true;
  }

 protected:
  std::optional<Phdr>& phdr() { return phdr_; }

 private:
  std::optional<Phdr>& phdr_;
};

// Observes PT_GNU_STACK metadata. If CanBeExecutable is true, then the absence
// of any PT_GNU_STACK header is conventionally taken to mean that the stack is
// executable; if CanBeExecutable is false, then Finish() gives an error if no
// header is found or if it reports that the stack is executable (i.e., if
// PF_X is set).
template <ElfApi Elf, bool CanBeExecutable = false>
class PhdrStackObserver : public PhdrSingletonObserver<Elf, ElfPhdrType::kStack> {
 private:
  using Base = PhdrSingletonObserver<Elf, ElfPhdrType::kStack>;
  using size_type = Elf::size_type;
  using Phdr = Elf::Phdr;

 public:
  // There is only one constructor, but its signature is dependent on
  // `CanBeExecutable`.

  template <bool E = CanBeExecutable, typename = std::enable_if_t<!E>>
  explicit PhdrStackObserver(std::optional<size_type>& size) : Base(phdr_), size_(size) {}

  template <bool E = CanBeExecutable, typename = std::enable_if_t<E>>
  PhdrStackObserver(std::optional<size_type>& size, bool& executable)
      : Base(phdr_), size_(size), executable_(executable) {}

  template <class Diagnostics>
  constexpr bool Finish(Diagnostics& diagnostics) {
    using namespace std::literals::string_view_literals;

    if (!phdr_) {
      if constexpr (CanBeExecutable) {
        executable_ = true;
        return true;
      } else {
        return diagnostics.FormatError(
            "executable stack not supported: PT_GNU_STACK header required"sv);
      }
    }

    const auto flags = phdr_->flags();
    if ((flags & Phdr::kRead) == 0 &&
        !diagnostics.FormatError("stack is not readable: PF_R is not set"sv)) {
      return false;
    }
    if ((flags & Phdr::kWrite) == 0 &&
        !diagnostics.FormatError("stack is not writable: PF_W is not set"sv)) {
      return false;
    }

    if constexpr (CanBeExecutable) {
      executable_ = (flags & Phdr::kExecute) != 0;
    } else {
      if ((flags & Phdr::kExecute) != 0 &&
          !diagnostics.FormatError("executable stack not supported: PF_X is set"sv)) {
        return false;
      }
    }

    if (phdr_->memsz != size_type{0}) {
      size_ = phdr_->memsz;
    }

    return true;
  }

 private:
  struct Empty {};

  std::optional<Phdr> phdr_;
  std::optional<size_type>& size_;
  ELFLDLTL_NO_UNIQUE_ADDRESS std::conditional_t<CanBeExecutable, bool&, Empty> executable_;
};

// A generic metadata, singleton observer that validates constraints around
// sizes, offset, address, and segment entry type.
template <ElfApi Elf, ElfPhdrType Type, typename EntryType = std::byte>
class PhdrMetadataObserver : public PhdrSingletonObserver<Elf, Type> {
 private:
  using Base = PhdrSingletonObserver<Elf, Type>;
  using Phdr = Elf::Phdr;

 public:
  using Base::Base;

  template <class Diagnostics>
  constexpr bool Finish(Diagnostics& diagnostics) {
    using PhdrError = internal::PhdrError<Type>;

    const std::optional<Phdr>& phdr = this->phdr();
    if (!phdr) {
      return true;
    }

    if (alignof(EntryType) > phdr->align() &&  //
        !diagnostics.FormatError(PhdrError::kIncompatibleEntryAlignment)) {
      return false;
    }

    // Note that `p_vaddr % p_align == 0` implies that
    // `p_offset % p_align == 0` by virtue of the general equivalence check
    // made in DecodePhdrs().
    if (phdr->vaddr() % phdr->align() != 0 &&  //
        !diagnostics.FormatError(PhdrError::kUnalignedVaddr)) {
      return false;
    }

    if (phdr->memsz != phdr->filesz &&  //
        !diagnostics.FormatError(PhdrError::kFileszNotEqMemsz)) {
      return false;
    }

    if (phdr->filesz() % sizeof(EntryType) != 0 &&  //
        !diagnostics.FormatError(PhdrError::kIncompatibleEntrySize)) {
      return false;
    }

    return true;
  }
};

template <ElfApi Elf>
using PhdrDynamicObserver = PhdrMetadataObserver<Elf, ElfPhdrType::kDynamic, typename Elf::Dyn>;

template <ElfApi Elf>
using PhdrInterpObserver = PhdrMetadataObserver<Elf, ElfPhdrType::kInterp>;

template <ElfApi Elf>
using PhdrEhFrameHdrObserver = PhdrMetadataObserver<Elf, ElfPhdrType::kEhFrameHdr>;

template <ElfApi Elf>
using PhdrRelroObserver = PhdrSingletonObserver<Elf, ElfPhdrType::kRelro>;

template <ElfApi Elf>
using PhdrTlsObserver = PhdrSingletonObserver<Elf, ElfPhdrType::kTls>;

// PT_LOAD validation policy. Subsequent values extend previous ones.
enum class PhdrLoadPolicy {
  // Universal checks for all phdrs are made here (beyond universal checks
  // already made in `DecodePhdr()`):
  // * `p_align` is runtime page-aligned.
  // * `p_memsz >= p_filesz`;
  // * `p_align`-aligned memory ranges (`[p_vaddr, p_vaddr + p_memsz)`) do
  //   not overlap and increase monotonically.
  //
  // Underspecified, pathological cases like where `p_vaddr + p_memsz` or
  // `p_offset + p_filesz` overflow are checked as well.
  kBasic,

  // kFileRangeMonotonic further asserts that
  // * `p_align`-aligned file offset ranges (`[p_offset, p_offset + p_filesz)`)
  //   do not overlap and increase monotonically.
  //
  // This condition is meaningful for an ELF loader because if a writable
  // segment overlaps with any other segment, then one needs to map the former
  // to a COW copy of that part of the file rather than reusing the file data
  // directly (assuming one cares to not mutate the latter segment).
  kFileRangeMonotonic,

  // kContiguous further asserts that there is maximal 'contiguity' in the file
  // and memory layouts:
  // * `p_align`-aligned memory ranges are contiguous
  // * `p_align`-aligned file offset ranges are contiguous
  // * The first `p_offset` lies in the first page.
  // The first two conditions ensure that the unused space between ranges is
  // minimal, and permit the ELF file to be loaded as whole. Moreover, when
  // loading as a whole, the last condition ensures minimality in the unused
  // space in between the ELF header and the first `p_offset`.
  kContiguous,
};

struct PhdrLoadNoCallback {
  template <class Diagnostics, class Phdr>
  std::true_type operator()(Diagnostics& diagnostics, const Phdr& phdr) const {
    return {};
  }
};

// A PT_LOAD observer for a given metadata policy.
template <ElfApi Elf, PhdrLoadPolicy Policy = PhdrLoadPolicy::kBasic,
          typename Callback = PhdrLoadNoCallback>
class PhdrLoadObserver
    : public PhdrObserver<PhdrLoadObserver<Elf, Policy, Callback>, ElfPhdrType::kLoad> {
 private:
  using Phdr = Elf::Phdr;
  using size_type = Elf::size_type;

 public:
  // `vaddr_start` and `vaddr_size` are updated to track the size of the
  // page-aligned memory image throughout observation.
  PhdrLoadObserver(size_type page_size, size_type& vaddr_start, size_type& vaddr_size,
                   Callback callback)
      : vaddr_start_(vaddr_start),
        vaddr_size_(vaddr_size),
        page_size_(page_size),
        callback_(std::move(callback)) {
    assert(std::has_single_bit(page_size));
    vaddr_start_ = 0;
    vaddr_size_ = 0;
    assert(NoHeadersSeen());
  }

  template <typename C = Callback, typename = std::enable_if_t<std::is_default_constructible_v<C>>>
  PhdrLoadObserver(size_type page_size, size_type& vaddr_start, size_type& vaddr_size)
      : PhdrLoadObserver(page_size, vaddr_start, vaddr_size, Callback{}) {}

  template <class Diag>
  constexpr bool Observe(Diag& diag, PhdrTypeMatch<ElfPhdrType::kLoad> type, const Phdr& phdr) {
    using namespace std::literals::string_view_literals;

    static_assert(std::is_invocable_r_v<bool, Callback, Diag&, const Phdr&>);
    auto success = [&]() {
      UpdateHighWatermarks(phdr);
      return std::invoke(callback_, diag, phdr);
    };

    // If `p_align` is not page-aligned, then this file cannot be loaded through
    // normal memory mapping.
    if (0 < phdr.align() && phdr.align() < page_size_ &&
        !diag.FormatError("PT_LOAD's `p_align` is not page-aligned"sv, phdr.align())) {
      return false;
    }

    if (phdr.memsz() == 0 && !diag.FormatWarning("PT_LOAD has `p_memsz == 0`"sv)) {
      return false;
    }

    if (phdr.memsz() < phdr.filesz() && !diag.FormatError("PT_LOAD has `p_memsz < p_filesz`"sv)) {
      return false;
    }

    // A `p_align` of 0 signifies no alignment constraints. So that we can
    // uniformally perform the usual alignment arithmetic below, we convert
    // such a value to 1, which has the same intended effect.
    auto align = std::max<size_type>(phdr.align(), 1);

    // Technically, having `p_vaddr + p_memsz` or `p_offset + p_filesz` be
    // exactly 2^64 is kosher per the ELF spec; however, such an ELF is not
    // usable, so we conventionally error out here along with at the usual
    // overflow scenarios.
    constexpr auto kMax = std::numeric_limits<size_type>::max();
    if (phdr.memsz() > kMax - phdr.vaddr()) [[unlikely]] {
      return diag.FormatError("PT_LOAD has overflowing `p_vaddr + p_memsz`"sv);
    }
    if (phdr.vaddr() + phdr.memsz() > kMax - align + 1) [[unlikely]] {
      return diag.FormatError("PT_LOAD has overflowing `p_align`-aligned `p_vaddr + p_memsz`"sv);
    }
    if (phdr.filesz() > kMax - phdr.offset()) [[unlikely]] {
      return diag.FormatError("PT_LOAD has overflowing `p_offset + p_filesz`"sv);
    }
    if (phdr.offset() + phdr.filesz() > kMax - align + 1) [[unlikely]] {
      return diag.FormatError("PT_LOAD has overflowing `p_align`-aligned `p_offset + p_filesz`"sv);
    }

    if (NoHeadersSeen()) {
      if constexpr (Policy == PhdrLoadPolicy::kContiguous) {
        if (phdr.offset() >= page_size_ &&
            !diag.FormatError("first PT_LOAD's `p_offset` does not lie within the first page"sv,
                              FileOffset{phdr.offset()})) [[unlikely]] {
          return false;
        }
      }

      vaddr_start_ = AlignDown(phdr.vaddr(), page_size_);
      vaddr_size_ = AlignUp(phdr.vaddr() + phdr.memsz(), page_size_) - vaddr_start_;

      return success();
    }

    if (AlignDown(phdr.vaddr(), align) < high_memory_watermark_) [[unlikely]] {
      return diag.FormatError(
          "PT_LOAD has `p_align`-aligned memory ranges that overlap or do not increase monotonically"sv);
    }

    if constexpr (kTrackFileOffsets) {
      if (AlignDown(phdr.offset(), align) < high_file_watermark_) [[unlikely]] {
        return diag.FormatError(
            "PT_LOAD has `p_align`-aligned file offset ranges that overlap or do not increase monotonically"sv);
      }
    }

    if constexpr (Policy == PhdrLoadPolicy::kContiguous) {
      if (AlignDown(phdr.vaddr(), align) != high_memory_watermark_) [[unlikely]] {
        return diag.FormatError(
            "PT_LOAD has `p_align`-aligned memory ranges that are not contiguous"sv);
      }

      if (AlignDown(phdr.offset(), align) != high_file_watermark_) [[unlikely]] {
        return diag.FormatError(
            "PT_LOAD has `p_align`-aligned file offset ranges that are not contiguous"sv);
      }
    }

    vaddr_size_ = AlignUp(phdr.vaddr() + phdr.memsz(), page_size_) - vaddr_start_;

    return success();
  }

  template <class Diag>
  constexpr bool Finish(Diag& diag) {
    return true;
  }

 private:
  struct Empty {};

  // Whether the given policy requires tracking file offsets.
  static constexpr bool kTrackFileOffsets = Policy != PhdrLoadPolicy::kBasic;

  static constexpr size_type AlignUp(size_type value, size_type alignment) {
    return (value + (alignment - 1)) & -alignment;
  }

  static constexpr size_type AlignDown(size_type value, size_type alignment) {
    return value & -alignment;
  }

  // Whether any PT_LOAD header have yet been observed.
  bool NoHeadersSeen() const { return vaddr_start_ == 0 && vaddr_size_ == 0; }

  void UpdateHighWatermarks(const Phdr& phdr) {
    auto align = std::max<size_type>(phdr.align(), 1);  // As above.
    high_memory_watermark_ = AlignUp(phdr.vaddr() + phdr.memsz(), align);

    if constexpr (kTrackFileOffsets) {
      high_file_watermark_ = AlignUp(phdr.offset() + phdr.filesz(), align);
    }
  }

  // The total, observed page-aligned load segment range in memory.
  size_type& vaddr_start_;
  size_type& vaddr_size_;

  // System page size.
  const size_type page_size_;

  // The highest `p_align`-aligned address and offset seen thus far.
  size_type high_memory_watermark_;
  ELFLDLTL_NO_UNIQUE_ADDRESS std::conditional_t<kTrackFileOffsets, size_type, Empty>
      high_file_watermark_;

  // Additional tail of observer function.
  ELFLDLTL_NO_UNIQUE_ADDRESS Callback callback_;
};

// This acts as a deduction guide with partial explicit specialization.
template <ElfApi Elf, PhdrLoadPolicy Policy = PhdrLoadPolicy::kBasic, typename T>
constexpr auto MakePhdrLoadObserver(typename Elf::size_type page_size,
                                    typename Elf::size_type& vaddr_start,
                                    typename Elf::size_type& vaddr_size, T&& callback) {
  using Obs = PhdrLoadObserver<Elf, Policy, std::decay_t<T>>;
  return Obs(page_size, vaddr_start, vaddr_size, std::forward<T>(callback));
}

// Given a set of phdrs, call the callback as `bool(size_t vaddr, size_t
// memsz)` to cover all the writable segment data described by PT_LOAD
// segments, excluding the RELRO region.  Returns false as soon as the first
// callback returns false, or else returns true.
template <class Phdr, typename T>
constexpr bool OnPhdrWritableSegments(std::span<const Phdr> phdrs, T&& callback) {
  using size_type = decltype(phdrs.front().memsz());
  static_assert(std::is_invocable_r_v<bool, T, size_type, size_type>);

  // First find the RELRO region, if any.  It will be excluded.
  const Phdr* relro = nullptr;
  for (const Phdr& phdr : phdrs) {
    if (phdr.type == elfldltl::ElfPhdrType::kRelro) {
      relro = &phdr;
      break;
    }
  }

  for (const Phdr& phdr : phdrs) {
    if (phdr.type == elfldltl::ElfPhdrType::kLoad && (phdr.flags & Phdr::kWrite)) {
      // This is a writable segment.
      size_type start = phdr.vaddr;
      size_type end = start + phdr.memsz;

      if (relro && relro->vaddr >= start && relro->vaddr <= end) {
        // If this segment contains the RELRO region, exclude that leading
        // range of the segment.  With LLD behavior, that's the entire segment
        // because RELRO gets a separate aligned segment.  With GNU behavior,
        // it's just a leading portion of the main writable segment.  LLD uses
        // a page-rounded p_memsz for PT_GNU_RELRO (with the actual size in
        // p_filesz) unlike GNU linkers (where p_memsz == p_filesz), so p_vaddr
        // + p_memsz might actually be past end.
        start = std::min(end, relro->vaddr + relro->memsz);
      }

      if (end > start && !callback(start, end - start)) {
        return false;
      }
    }
  }

  return true;
}

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_PHDR_H_
