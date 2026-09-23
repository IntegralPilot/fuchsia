// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_PAGE_ALLOCATOR_H_
#define SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_PAGE_ALLOCATOR_H_

#include <zircon/compiler.h>

#include <concepts>
#include <cstddef>
#include <limits>
#include <type_traits>
#include <utility>

namespace trivial_allocator {

// A memory resource that can be allocated and leaked.
template <typename T>
concept MemoryApi =
    std::movable<typename T::Capability> &&
    std::is_default_constructible_v<typename T::Capability> &&
    requires(const T const_memory, T memory, typename T::Capability cap, void* alloc, size_t size) {
      // The page_size() returned must be a power of two.
      { const_memory.page_size() } -> std::convertible_to<std::size_t>;

      // Allocate `size` bytes, where size is a multiple of `const_memory.page_size()`, and return
      // a pointer to the allocation and a capability gating access to such allocation, where
      // supported.
      { memory.Allocate(size) } -> std::convertible_to<std::pair<void*, typename T::Capability>>;

      // Free `size` bytes pointed by `alloc`.
      { memory.Deallocate(std::move(cap), alloc, size) };

      // Leak `size` bytes pointed by `alloc`.
      { memory.Release(std::move(cap), alloc, size) };
    };

// A memory representation that can become read-only.
template <typename T>
concept SealableMemory =
    MemoryApi<T> && requires(T memory, typename T::Capability cap, void* alloc, size_t size) {
      // Seal `size` bytes pointed by `alloc`, and the leak them. Sealed memory is `read-only`.
      { memory.Seal(std::move(cap), alloc, size) };
    };

// trivial_allocator::PageAllocator is an AllocateFunction compatible with
// trivial_allocator::BasicLeakyAllocator.  It uses the Memory object to do
// whole-page allocations.
//
// Its constructor forwards arguments to the Memory constructor, so the PageAllocator object is
// copyable and/or movable if the Memory object is.
//
// Some Memory object implementations are provided by:
//    * <lib/trivial-allocator/zircon.h>
//    * <lib/trivial-allocator/posix.h>
template <MemoryApi Memory>
class PageAllocator {
 public:
  class Allocation {
   public:
    Allocation() = default;

    Allocation(const Allocation&) = delete;

    Allocation(Allocation&& other) noexcept
        : allocator_(std::exchange(other.allocator_, nullptr)),
          capability_(std::exchange(other.capability_, {})),
          ptr_(std::exchange(other.ptr_, nullptr)),
          size_(std::exchange(other.size_, 0)) {}

    Allocation& operator=(const Allocation&) = delete;

    Allocation& operator=(Allocation&& other) noexcept {
      reset();
      allocator_ = std::exchange(other.allocator_, nullptr);
      capability_ = std::exchange(other.capability_, {});
      ptr_ = std::exchange(other.ptr_, nullptr);
      size_ = std::exchange(other.size_, 0);
      return *this;
    }

    ~Allocation() { reset(); }

    void* get() const { return ptr_; }

    explicit operator bool() const { return ptr_; }

    size_t size_bytes() const { return size_; }

    void reset() {
      if (ptr_) {
        allocator_->memory().Deallocate(std::exchange(capability_, {}),
                                        std::exchange(ptr_, nullptr), std::exchange(size_, 0));
      }
    }

    void* release() {
      allocator_->memory().Release(std::exchange(capability_, {}), ptr_, std::exchange(size_, 0));
      return std::exchange(ptr_, nullptr);
    }

    // Seal the memory and then leak it.
    void Seal() &&
      requires SealableMemory<Memory>
    {
      allocator_->memory().Seal(std::exchange(capability_, {}), std::exchange(ptr_, nullptr),
                                std::exchange(size_, 0));
    }

    PageAllocator& allocator() const { return *allocator_; }

   private:
    friend PageAllocator;

    PageAllocator* allocator_ = nullptr;
    __NO_UNIQUE_ADDRESS typename Memory::Capability capability_;
    void* ptr_ = nullptr;
    size_t size_ = 0;
  };
  static_assert(std::is_default_constructible_v<Allocation>);
  static_assert(std::is_nothrow_move_constructible_v<Allocation>);
  static_assert(std::is_nothrow_move_assignable_v<Allocation>);
  static_assert(!std::is_copy_constructible_v<Allocation>);
  static_assert(!std::is_copy_assignable_v<Allocation>);

  constexpr PageAllocator() = default;
  constexpr PageAllocator(const PageAllocator&) = default;
  constexpr PageAllocator(PageAllocator&&) noexcept = default;

  constexpr PageAllocator& operator=(PageAllocator&&) noexcept = default;

  template <typename... Args>
  constexpr explicit PageAllocator(Args&&... args) : memory_(std::forward<Args>(args)...) {}

  Allocation operator()(size_t& size, size_t alignment) {
    Allocation result;
    if (size <= std::numeric_limits<size_t>::max() - memory_.page_size() + 1) [[likely]] {
      size = (size + memory_.page_size() - 1) & -memory_.page_size();
      auto [ptr, capability] = memory_.Allocate(size);
      if (ptr) {
        result.allocator_ = this;
        result.capability_ = std::move(capability);
        result.ptr_ = ptr;
        result.size_ = size;
      }
    }
    return result;
  }

  Memory& memory() { return memory_; }
  const Memory& memory() const { return memory_; }

 private:
  Memory memory_;
};

// Deduction guide.
template <MemoryApi Memory>
PageAllocator(Memory) -> PageAllocator<std::decay_t<Memory>>;

}  // namespace trivial_allocator

#endif  // SRC_LIB_TRIVIAL_ALLOCATOR_INCLUDE_LIB_TRIVIAL_ALLOCATOR_PAGE_ALLOCATOR_H_
