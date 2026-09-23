// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef HWREG_ARRAY_H_
#define HWREG_ARRAY_H_

#include <lib/stdcompat/span.h>

#include <array>
#include <atomic>
#include <type_traits>

#include "internal.h"

namespace hwreg {

// An hwreg::ArrayIo is an IoProvider for use with "bitfields.h" types
// that simply holds a std::span of the underlying integer type and does normal
// memory access on it. This can be either a fixed-size or a dynamic span. The
// `offset` arguments to the hwreg API methods are used as the index into the
// span. The templated methods require that the type parameter in the access
// (i.e. the underlying type of the given hwreg type) be exactly ElementType.
//
// For example:
// ```
// using PageTableIo = hwreg::ArrayIo<PageTableEntry::ValueType, 512>;
// PageTableIo io{page_table_array};
// auto pte = PageTableEntry::Get(pfn);
// pte.set_p(false);
// pte.WriteTo(&io);
// ```
template <typename ElementType, size_t ElementCount = cpp20::dynamic_extent,
          typename Ref = ElementType&>
class ArrayIo {
 public:
  using Span = cpp20::span<ElementType, ElementCount>;

  constexpr explicit ArrayIo(Span array) : array_(array) {}

  constexpr Span get() const { return array_; }

  template <typename IntType>
  constexpr void Write(IntType val, uint32_t offset) const {
    static_assert(std::is_same_v<IntType, ElementType>,
                  "hwreg::ArrayIo<T> supports Write<T>, not Write<OtherT>");
    At(offset) = val;
  }

  template <typename IntType>
  constexpr IntType Read(uint32_t offset) const {
    static_assert(std::is_same_v<IntType, ElementType>,
                  "hwreg::ArrayIo<T> supports Read<T>, not Read<OtherT>");
    return static_cast<ElementType>(At(offset));
  }

 private:
  constexpr Ref At(uint32_t offset) const {
    if constexpr (std::is_reference_v<Ref>) {
      return static_cast<Ref>(array_[offset]);
    } else {
      return Ref{array_[offset]};
    }
  }

  Span array_;
};

template <typename ElementType, size_t ElementCount>
ArrayIo(cpp20::span<ElementType, ElementCount>) -> ArrayIo<ElementType, ElementCount>;

// hwreg::AtomicArray<MO>::Io<...> is like hwreg::ArrayIo<...> but it uses
// cpp20::atomic_ref load and store methods with the MO argument to access
// the elements in the span.
template <std::memory_order MemoryOrder>
struct AtomicArray {
  // This can't just be an alias since then it couldn't have a deduction guide.
  template <typename ElementType, size_t ElementCount = cpp20::dynamic_extent>
  struct Io : public ArrayIo<ElementType, ElementCount,
                             internal::AtomicArrayIoRef<ElementType, MemoryOrder>> {
    using ArrayIo<ElementType, ElementCount,
                  internal::AtomicArrayIoRef<ElementType, MemoryOrder>>::ArrayIo;
  };

  template <typename ElementType, size_t ElementCount>
  Io(cpp20::span<ElementType, ElementCount>) -> Io<ElementType, ElementCount>;
};

// Convenience type for an array that's naturally-aligned to its whole size.
// It's zero-initialized if declared or used with new (e.g. placement new).
// It will usually be used via pointers to pre-existing aligned memory such as
// a page table.  The main purpose of the type is to manufacture ArrayIo or
// AtomicArrayIo accessors for using elements as the underlying integer type
// for hwreg register types.
template <typename ElementType, size_t ElementCount>
struct AlignedTableStorage {
  constexpr auto table() { return cpp20::span(table_); }

  constexpr auto direct_io() { return hwreg::ArrayIo(table()); }

  constexpr auto atomic_io() { return hwreg::AtomicArray<std::memory_order_relaxed>::Io(table()); }

 private:
  using Table = std::array<ElementType, ElementCount>;

  alignas(sizeof(Table)) Table table_{};
};

}  // namespace hwreg

#endif  // HWREG_ARRAY_H_
