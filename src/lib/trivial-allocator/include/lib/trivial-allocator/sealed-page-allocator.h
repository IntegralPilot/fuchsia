// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_SEALED_PAGE_ALLOCATOR_H_
#define SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_SEALED_PAGE_ALLOCATOR_H_

#include <array>
#include <type_traits>
#include <utility>

#include "page-allocator.h"

namespace trivial_allocator {

// trivial_allocator::SealedPageAllocator is a variant of PageAllocator that
// automatically "seals" its allocations to make them read-only.
//
// The release() method on the "smart pointer" object it returns has a special
// behavior: it moves that allocation to a "staging area" in the allocator.
// The staging area has space for at least one allocation, and as many more as
// the Reserve template argument requests.  While "released" allocations are in
// the staging area, they are writable.  When old allocations are evicted from
// the staging area to make room for a new allocation's release(), they get
// "sealed" so their memory is read-only, and then they are leaked.  Just
// before destruction, the Seal() method can be called to seal (and leak) the
// last allocations still in flight.  If Seal() is not called, any unsealed
// allocations still outstanding will be deallocated on destruction.

template <SealableMemory Memory, size_t Reserve = 0>
class SealedPageAllocator : public PageAllocator<Memory> {
 public:
  using Base = PageAllocator<Memory>;

  class Allocation {
   public:
    constexpr Allocation() = default;
    constexpr Allocation(const Allocation&) = delete;
    constexpr Allocation(Allocation&&) noexcept = default;

    constexpr Allocation& operator=(Allocation&&) noexcept = default;

    constexpr SealedPageAllocator& allocator() const {
      return static_cast<SealedPageAllocator&>(allocation_.allocator());
    }

    constexpr void* get() const { return allocation_.get(); }

    constexpr explicit operator bool() const { return static_cast<bool>(allocation_); }

    constexpr size_t size_bytes() const { return allocation_.size_bytes(); }

    constexpr void reset() { allocation_.reset(); }

    // When the allocation is "released", actually move it to the reserve list.
    constexpr void* release() { return allocator().ReleaseToReserve(std::move(*this)); }

   private:
    friend SealedPageAllocator;

    typename Base::Allocation allocation_;
  };

  using Base::Base;

  constexpr SealedPageAllocator(SealedPageAllocator&&) noexcept = default;

  constexpr SealedPageAllocator& operator=(SealedPageAllocator&&) noexcept = default;

  constexpr Allocation operator()(size_t& size, size_t alignment) {
    Allocation result;
    result.allocation_ = Base::operator()(size, alignment);
    return result;
  }

  constexpr void Seal() && {
    for (Allocation& slot : unsealed_) {
      if (slot) {
        std::exchange(slot, {}).allocation_.Seal();
      }
    }
  }

 private:
  using ReserveArray = std::array<Allocation, Reserve + 1>;
  using ReserveIndex = std::conditional_t<Reserve == 0, std::integral_constant<size_t, 0>, size_t>;

  constexpr Allocation& NextReserveSlot() {
    Allocation& reserve_slot = unsealed_[unsealed_idx_];
    if constexpr (Reserve > 0) {
      ++unsealed_idx_;
      unsealed_idx_ %= unsealed_.size();
    }
    return reserve_slot;
  }

  constexpr void* ReleaseToReserve(Allocation&& released) {
    // Stash the most recently released allocation in the next slot.
    Allocation& reserve_slot = NextReserveSlot();
    if (Allocation old = std::exchange(reserve_slot, std::move(released))) {
      // The slot was already occupied, meaning the reserve is now full.
      // Seal (and leak) the old allocation whose slot was just taken.
      std::move(old).allocation_.Seal();
    }
    return reserve_slot.get();
  }

  [[no_unique_address]] ReserveArray unsealed_;
  [[no_unique_address]] ReserveIndex unsealed_idx_{};
};

}  // namespace trivial_allocator

#endif  // SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_SEALED_PAGE_ALLOCATOR_H_
