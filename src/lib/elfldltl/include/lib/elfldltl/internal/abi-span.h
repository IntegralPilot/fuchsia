// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_ABI_SPAN_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_ABI_SPAN_H_

#include <cassert>
#include <span>
#include <type_traits>

#include "../abi-ptr.h"
#include "../layout.h"

namespace elfldltl {

// Forward declaration.
template <typename T, size_t N, ElfApi Elf, AbiPtrTraitsApi<T, Elf> Traits>
class AbiSpan;

namespace internal {

// This is the common base type for all AbiSpan instantiations.
// It's separately instantiated for each one, but then different
// subclasses are defined for different instantiations.
template <typename T, size_t N, ElfApi Elf, AbiPtrTraitsApi<T, Elf> Traits>
class AbiSpanImplBase {
 public:
  using Ptr = AbiPtr<T, Elf, Traits>;
  using Addr = Ptr::Addr;
  using size_type = Ptr::size_type;

  using element_type = T;
  using value_type = std::remove_cv_t<T>;
  using difference_type = std::make_signed_t<size_type>;

  static constexpr size_t extent = N;

  constexpr const Ptr& ptr() const { return ptr_; }

  // These are the basic methods always available.  They rely on the size()
  // method defined by the final AbiSpan instantiation.

  constexpr bool empty() const { return AsSpan().size() == 0; }

  constexpr size_t size_bytes() const { return AsSpan().size() * sizeof(T); }

  template <size_t Count>
  constexpr auto first() const {
    assert(Count <= AsSpan().size());
    return Span<Count>{ptr_, Count};
  }

  constexpr auto first(size_type n) const {
    assert(n <= AsSpan().size());
    return Span<>{ptr_, n};
  }

  template <size_t Count>
  constexpr auto last() const {
    assert(Count <= AsSpan().size());
    return Span<Count>{ptr_ + (AsSpan().size() - Count), Count};
  }

  constexpr auto last(size_type n) const {
    assert(n <= AsSpan().size());
    return Span<>{ptr_ + (AsSpan().size() - n), n};
  }

  template <size_t Offset, size_t Count = std::dynamic_extent>
  constexpr auto subspan() const {
    static_assert(Offset <= N);
    assert(Offset <= AsSpan().size());
    if constexpr (Count == std::dynamic_extent) {
      if constexpr (N == std::dynamic_extent) {
        return Span<>{
            ptr_ + Offset,
            static_cast<size_type>(AsSpan().size() - Offset),
        };
      } else {
        constexpr size_t SubCount = std::min(Count, N - Offset);
        return Span<SubCount>{ptr_ + Offset, static_cast<size_type>(SubCount)};
      }
    } else {
      assert(Count <= AsSpan().size() - Offset);
      return Span<Count>{ptr_ + Offset, static_cast<size_type>(Count)};
    }
  }

  constexpr auto subspan(size_type offset,
                         size_type count = static_cast<size_type>(std::dynamic_extent)) const {
    assert(offset <= AsSpan().size());
    if (count == static_cast<size_type>(std::dynamic_extent)) {
      count = AsSpan().size() - offset;
    } else {
      assert(count <= AsSpan().size() - offset);
    }
    return Span<>{ptr_ + offset, count};
  }

 protected:
  // This is the actual end-user instantiation being created.
  template <size_t Extent = N>
  using Span = AbiSpan<T, Extent, Elf, Traits>;

  constexpr AbiSpanImplBase() = default;

  constexpr AbiSpanImplBase(const AbiSpanImplBase&) = default;

  constexpr explicit AbiSpanImplBase(const Ptr& ptr) : ptr_(ptr) {}

  constexpr AbiSpanImplBase& operator=(const AbiSpanImplBase&) = default;

  constexpr auto& AsSpan() const { return *static_cast<const Span<>*>(this); }

 private:
  Ptr ptr_;
};

// If AbiPtr::get() isn't supported, no access methods are provided.
template <typename T, size_t N, ElfApi Elf, AbiPtrTraitsApi<T, Elf> Traits,
          bool Local = AbiPtrLocalTraitsApi<Traits, T, Elf>>
  requires(Local == AbiPtrLocalTraitsApi<Traits, T, Elf>)
class AbiSpanImpl : public AbiSpanImplBase<T, N, Elf, Traits> {
 public:
  using AbiSpanImplBase<T, N, Elf, Traits>::AbiSpanImplBase;
  using AbiSpanImplBase<T, N, Elf, Traits>::operator=;
};

// This specialization kicks in when AbiPtr::get() is available.
template <typename T, size_t N, ElfApi Elf, class Traits>
class AbiSpanImpl<T, N, Elf, Traits, true> : public AbiSpanImplBase<T, N, Elf, Traits> {
 public:
  using pointer = T*;
  using const_pointer = const T*;
  using reference = T&;
  using const_reference = const T&;
  using iterator = std::span<T, N>::iterator;
  using reverse_iterator = std::span<T, N>::reverse_iterator;

  using AbiSpanImplBase<T, N, Elf, Traits>::AbiSpanImplBase;
  using AbiSpanImplBase<T, N, Elf, Traits>::AsSpan;
  using AbiSpanImplBase<T, N, Elf, Traits>::operator=;
  using AbiSpanImplBase<T, N, Elf, Traits>::ptr;

  constexpr T* data() const { return ptr().get(); }

  constexpr std::span<T, N> get() const {
    const auto count = AsSpan().size();
    return std::span<T, N>{data(), count};
  }

  constexpr decltype(auto) front() const { return get().front(); }

  constexpr decltype(auto) back() const { return get().back(); }

  constexpr decltype(auto) operator[](size_t i) const { return get()[i]; }

  constexpr iterator begin() const { return get().begin(); }

  constexpr iterator end() const { return get().end(); }

  constexpr reverse_iterator rbegin() const { return get().rbegin(); }

  constexpr reverse_iterator rend() const { return get().rend(); }
};

}  // namespace internal
}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_ABI_SPAN_H_
