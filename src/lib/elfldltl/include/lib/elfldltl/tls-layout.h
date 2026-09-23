// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_TLS_LAYOUT_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_TLS_LAYOUT_H_

#include <algorithm>
#include <bit>
#include <cassert>
#include <compare>
#include <concepts>
#include <type_traits>

#include "constants.h"
#include "layout.h"
#include "machine.h"

namespace elfldltl {

// This performs static TLS layout according to the machine's ABI rules.
//
// Each call to Assign takes the PT_TLS phdr for a module and returns the
// thread pointer offset for that module's TLS block.  If there is a main
// executable that might use the Local Exec TLS model, then its TLS block must
// be assigned first.
//
// When all the static TLS modules have been assigned, then size_bytes() and
// alignment() return the size and alignment for the static TLS area.  The size
// is zero if there were no nonempty PT_TLS segments passed to Assign at all.
// When nonzero, the size includes the ABI reserved area, if any: it's the
// space that must be available immediately at (or before) the thread pointer.

template <class Elf = Elf<>>
class TlsLayout {
 public:
  using Addr = Elf::Addr;
  using size_type = Elf::size_type;
  using Phdr = Elf::Phdr;

  // A default-constructed TlsLayout is trivially zero-initialized and ready to
  // be updated using the Assign() method.
  constexpr TlsLayout() = default;

  // Construct a pre-existing layout from another source.  Unless zero, this
  // size_bytes must already include any ABI-specified kTlsLocalExecOffset.
  constexpr TlsLayout(Addr size_bytes, Addr alignment)
      : size_bytes_(size_bytes), alignment_(alignment) {
    assert(std::has_single_bit(alignment_()));
    static_assert(std::is_trivially_copyable_v<TlsLayout>);
    static_assert(std::is_trivially_copy_assignable_v<TlsLayout>);
    static_assert(std::is_trivially_destructible_v<TlsLayout>);
    static_assert(std::equality_comparable<TlsLayout>);
    static_assert(std::three_way_comparable<TlsLayout>);
  }

  // Sortable first on size, and secondarily on alignment.
  // The == and != operators are implicitly defaulted.
  constexpr auto operator<=>(const TlsLayout&) const = default;

  template <ElfMachine Machine = ElfMachine::kNative, size_type RedZone = 0>
  constexpr size_type Assign(const Phdr& phdr) {
    return Assign<Machine, RedZone>(phdr.memsz, phdr.align);
  }

  template <ElfMachine Machine = ElfMachine::kNative, size_type RedZone = 0>
  constexpr size_type Assign(size_type segment_size, size_type segment_alignment) {
    using Traits = TlsTraits<Elf, Machine>;

    segment_alignment = std::max<size_type>(segment_alignment, 1);
    assert(std::has_single_bit(segment_alignment));

    // The first module gets assigned at a fixed offset.  This isn't just made
    // the initializer value for size_bytes_ for two reasons: to keep this type
    // purely zero-initialized so it can live in .bss; so that the state when
    // no PT_TLS segments exist at all is always simply zero size.
    if (size_bytes_ == 0) {
      size_bytes_ = Traits::kTlsLocalExecOffset;
    }

    // The whole static TLS block must be at least as aligned as each segment.
    alignment_ = std::max<size_type>(alignment_, segment_alignment);

    // Within the block, each segment must be aligned according to its p_align.
    auto segment_aligned = [segment_alignment](size_type size) {
      return AlignUp(size, segment_alignment);
    };

    // Assign an offset for this segment and update the total size.
    if constexpr (Traits::kTlsNegative) {
      // Below the last assignment, aligned down as needed.
      size_type offset = segment_aligned(size_bytes_ + segment_size);
      size_bytes_ = offset + RedZone;
      return -offset;
    } else {
      // Above the last assignment, aligned up as needed.
      size_type offset = segment_aligned(size_bytes_);
      size_bytes_ = offset + segment_size + RedZone;
      return offset;
    }
  }

  constexpr size_type size_bytes() const { return size_bytes_; }

  constexpr size_type alignment() const { return alignment_; }

  // This rounds up a size to at least alignment(); and also to at least the
  // second argument if given.
  constexpr size_type Align(size_type size, size_type min_alignment = 1) const {
    assert(min_alignment > 0);
    return AlignUp(size, std::max<size_type>(min_alignment, alignment_));
  }

 private:
  static constexpr size_type AlignUp(size_type size, size_type alignment) {
    return (size + alignment - 1) & -alignment;
  }

  // *Note:* The member order matters for operator<=> ordering.
  Addr size_bytes_ = 0;
  Addr alignment_ = 0;

 public:
  // <lib/ld/remote-abi-transcriber.h> introspection API.  These aliases must
  // be public, but can't be defined lexically before the private: section that
  // declares the members; so this special public: section is at the end.

  using AbiLocal = TlsLayout;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers = Template<&TlsLayout::size_bytes_, &TlsLayout::alignment_>;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_TLS_LAYOUT_H_
