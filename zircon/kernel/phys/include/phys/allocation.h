// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ALLOCATION_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ALLOCATION_H_

#include <lib/fit/result.h>
#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>

#include <fbl/alloc_checker.h>
#include <ktl/byte.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <ktl/utility.h>
#include <phys/stdio.h>

// This object represents one memory allocation, and owns that allocation so
// destroying this object frees the allocation.  It acts as a smart pointer
// that also knows the size so it can deliver a raw pointer or a span<byte>.
class Allocation {
 public:
  // A default-constructed object is like a null pointer.
  // Allocation::New() must be called to create a non-null Allocation.
  Allocation() = default;

  Allocation(const Allocation&) = delete;

  Allocation(Allocation&& other) noexcept { *this = ktl::move(other); }

  Allocation& operator=(const Allocation&) = delete;

  Allocation& operator=(Allocation&& other) noexcept {
    ktl::swap(data_, other.data_);
    ktl::swap(alignment_, other.alignment_);
    ktl::swap(type_, other.type_);
    return *this;
  }

  ~Allocation() { reset(); }

  // This must be called exactly once before using GetPool or New.
  static void Init(ktl::span<memalloc::Range> mem_ranges, ktl::span<memalloc::Range> special_ranges,
                   memalloc::Pool::AccessCallback access_callback = {});

  // Alternatively, this can be called instead of Init() to install a
  // previously-initialized memalloc::Pool that was handed off.
  static void InitWithPool(memalloc::Pool& pool);

  // Turns `range` into an allocation object, taking ownership of the allocation.
  //
  // The range's base addr MUST BE page aligned, such that caller is required to guarantee
  // that the entire page belongs to the same allocation.
  //
  // It is the caller's responsibility to ensure that there is not more than a single owner.
  [[nodiscard]] static Allocation Adopt(memalloc::Range range) {
    ZX_ASSERT(range.addr % PageSize() == 0);

    Allocation alloc;
    alloc.type_ = range.type;
    alloc.data_ = {reinterpret_cast<ktl::byte*>(range.addr), static_cast<size_t>(range.size)};
    alloc.alignment_ = PageSize();

    // Try to extend to bounding page.
    if (alloc->size_bytes() % PageSize() != 0) {
      // This allows payloads to be extended in place and updates the book keeping to reflect that.
      fbl::AllocChecker ac;
      alloc.Extend(ac, fbl::round_up(alloc->size_bytes(), PageSize()));

      // Adopted ranges should NOT be sharing pages with other payloads.
      if (!ac.check()) {
        debugf("Warning: Adopted allocation cannot be extended to page boundary.\n");
      }
    }

    return alloc;
  }

  // If allocation fails, operator bool will return false later.
  // The AllocChecker must be checked after construction, too.
  [[nodiscard]] static Allocation New(fbl::AllocChecker& ac, memalloc::Type type, size_t size,
                                      size_t alignment = __STDCPP_DEFAULT_NEW_ALIGNMENT__,
                                      ktl::optional<uint64_t> min_addr = ktl::nullopt,
                                      ktl::optional<uint64_t> max_addr = ktl::nullopt);

  // Get the memalloc::Pool instance used to construct Allocation objects.
  // Every call returns the same object.  Note that a separate `#include
  // <lib/memalloc/pool.h>` is necessary to use the instance.
  [[gnu::const]] static memalloc::Pool& GetPool();

  // Size of pages used by the underlying allocator.
  [[gnu::const]] static size_t PageSize();

  ktl::span<ktl::byte> data() const { return data_; }

  size_t size_bytes() const { return data_.size(); }

  auto get() const { return data_.data(); }

  // Gives the intended minimal alignment.
  size_t alignment() const { return alignment_; }

  memalloc::Type type() const { return type_; }

  void reset();

  // This returns the span like data() but transfers ownership like a move.
  [[nodiscard]] auto release() {
    auto result = data_;
    data_ = {};
    alignment_ = 0;
    type_ = memalloc::Type::kMaxAllocated;
    return result;
  }

  void Resize(fbl::AllocChecker& ac, size_t new_size);

  // Attempts to grow the allocation to `new_size`, by extending the tail of the
  // associated range.
  //
  // `ac` will determined whether the allocation was successful or not.
  void Extend(fbl::AllocChecker& ac, size_t new_size);

  explicit operator bool() const { return !data_.empty(); }

  ktl::span<ktl::byte> operator*() const { return data_; }

  const ktl::span<ktl::byte>* operator->() const { return &data_; }

 private:
  ktl::span<ktl::byte> data_;
  size_t alignment_ = 0;
  memalloc::Type type_ = memalloc::Type::kMaxAllocated;
};

// Memory concept implementation over `Allocation` for `trivial_allocator::PageAllocator`.
class AllocationMemory {
 public:
  using Capability = Allocation;

  size_t page_size() const { return Allocation::PageSize(); }

  template <memalloc::Type MemoryType>
  std::pair<void*, Capability> Allocate(size_t size) {
    fbl::AllocChecker ac;
    auto alloc = Capability::New(ac, MemoryType, size);
    if (!ac.check()) {
      ZX_DEBUG_ASSERT(!alloc);
      return {};
    }
    ZX_DEBUG_ASSERT(alloc);
    return {alloc->data(), std::move(alloc)};
  }

  void Deallocate(Capability capability, void* address, size_t size) { capability.reset(); }

  void Release(Capability capability, void* address, size_t size) {
    ktl::ignore = capability.release();
  }
};

// Helper tor templating only on `Allocate` method.
template <memalloc::Type MemoryType>
class TypedMemoryAllocation : public AllocationMemory {
 public:
  std::pair<void*, Capability> Allocate(size_t size) {
    return AllocationMemory::template Allocate<MemoryType>(size);
  }
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ALLOCATION_H_
