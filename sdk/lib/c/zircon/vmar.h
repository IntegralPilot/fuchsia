// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_ZIRCON_VMAR_H_
#define LIB_C_ZIRCON_VMAR_H_

#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <sys/uio.h>

#include <cassert>
#include <cstddef>
#include <optional>
#include <span>
#include <utility>

#include "src/__support/macros/config.h"
#include "src/__support/math_extras.h"

namespace LIBC_NAMESPACE_DECL {

// This is the VMAR to be used for general data allocations and mappings.
// **Note:** It should not be presumed to permit executable mappings.
inline zx::unowned_vmar AllocationVmar() { return zx::unowned_vmar{_zx_vmar_root_self()}; }

// This wraps size_t to ensure that a size is always rounded to whole pages.
//
// To support safe chaining of operations with overflow detection, this class
// provides overloaded operators that work with std::optional<PageRoundedSize>.
// Normal arithmetic operators which could overflow return std::optional<PageRoundedSize>
// which will have a valid result when no overflow is detected.
//
// Examples:
//
// 1. Adding two PageRoundedSize objects:
//    PageRoundedSize a = ...;
//    PageRoundedSize b = ...;
//    std::optional<PageRoundedSize> sum = a + b;
//    if (!sum) { /* handle overflow */ }
//
// 2. Chaining multiple additions:
//    PageRoundedSize a = ...;
//    PageRoundedSize b = ...;
//    PageRoundedSize c = ...;
//    std::optional<PageRoundedSize> total = a + b + c;
//    if (!total) { /* handle overflow */ }
//
// 3. Adding a raw size (auto-rounded):
//    PageRoundedSize a = ...;
//    size_t raw_size = ...;
//    std::optional<PageRoundedSize> total = a + raw_size;
//
// 4. Construction from raw size with check:
//    std::optional<PageRoundedSize> size = PageRoundedSize::From(raw_size);
//
// 5. Construction from page count:
//    std::optional<PageRoundedSize> size = PageRoundedSize::Pages(num_pages);
//
class PageRoundedSize {
 public:
  constexpr PageRoundedSize() = default;
  constexpr PageRoundedSize(const PageRoundedSize&) = default;
  constexpr PageRoundedSize& operator=(const PageRoundedSize&) = default;

  static std::optional<PageRoundedSize> From(size_t raw_size) {
    if (raw_size == 0) {
      return PageRoundedSize{};
    }
    const size_t page_size = zx_system_get_page_size();
    size_t rounded;
    if (add_overflow(raw_size, page_size - 1, rounded)) {
      return std::nullopt;
    }
    rounded &= -page_size;
    return PageRoundedSize{rounded};
  }

  constexpr auto operator<=>(const PageRoundedSize&) const = default;

  constexpr size_t get() const { return rounded_size_; }

  explicit constexpr operator bool() const { return rounded_size_ > 0; }

  [[gnu::const]] static PageRoundedSize Page() {
    return PageRoundedSize{zx_system_get_page_size()};
  }

  static std::optional<PageRoundedSize> Pages(size_t num) { return Page() * num; }

  PageRoundedSize operator/(size_t other) const { return PageRoundedSize{rounded_size_ / other}; }

  friend constexpr std::optional<PageRoundedSize> operator+(PageRoundedSize a, PageRoundedSize b) {
    return !add_overflow(a.rounded_size_, b.rounded_size_, a.rounded_size_) ? std::optional{a}
                                                                            : std::nullopt;
  }

  friend constexpr std::optional<PageRoundedSize> operator+(std::optional<PageRoundedSize> a,
                                                            std::optional<PageRoundedSize> b) {
    return (a && b && !add_overflow(a->rounded_size_, b->rounded_size_, a->rounded_size_))
               ? a
               : std::nullopt;
  }

  friend constexpr std::optional<PageRoundedSize> operator+(std::optional<PageRoundedSize> a,
                                                            PageRoundedSize b) {
    return (a && !add_overflow(a->rounded_size_, b.rounded_size_, a->rounded_size_)) ? a
                                                                                     : std::nullopt;
  }

  friend constexpr std::optional<PageRoundedSize> operator+(PageRoundedSize a,
                                                            std::optional<PageRoundedSize> b) {
    return b + a;
  }

  friend std::optional<PageRoundedSize> operator+(std::optional<PageRoundedSize> a, size_t b) {
    return a + PageRoundedSize::From(b);
  }

  friend constexpr std::optional<PageRoundedSize> operator-(std::optional<PageRoundedSize> a,
                                                            std::optional<PageRoundedSize> b) {
    return (a && b && !sub_overflow(a->rounded_size_, b->rounded_size_, a->rounded_size_))
               ? a
               : std::nullopt;
  }

  friend constexpr std::optional<PageRoundedSize> operator*(std::optional<PageRoundedSize> a,
                                                            std::optional<PageRoundedSize> b) {
    return (a && b && !mul_overflow(a->rounded_size_, b->rounded_size_, a->rounded_size_))
               ? a
               : std::nullopt;
  }

  friend constexpr std::optional<PageRoundedSize> operator*(PageRoundedSize a, size_t b) {
    return !mul_overflow(a.rounded_size_, b, a.rounded_size_) ? std::optional{a} : std::nullopt;
  }

 private:
  friend class GuardedPageBlock;
  friend class ThreadStorage;

  explicit constexpr PageRoundedSize(size_t rounded_size) : rounded_size_(rounded_size) {}

  // TODO(https://fxbug.dev/510381428): math_extras.h in libc doesn't provide a mul_overflow.
  // We should replace this once llvm-libc provides it.
  template <typename T>
  static constexpr bool mul_overflow(T a, T b, T& res) {
    return __builtin_mul_overflow(a, b, &res);
  }

  size_t rounded_size_ = 0;
};

// This manages a VMO for use with GuardedPageBlock.  A single VMO is created
// to hold all the pages that will be mapped into separate blocks.
struct AllocationVmo {
  static zx::result<AllocationVmo> New(PageRoundedSize total_size) {
    AllocationVmo vmo;
    zx_status_t status = zx::vmo::create(total_size.get(), 0, &vmo.vmo);
    if (status != ZX_OK) [[unlikely]] {
      return zx::error{status};
    }
    return zx::ok(std::move(vmo));
  }

  uint64_t offset = 0;
  zx::vmo vmo;
};

// This describes a page-aligned block mapped inside a VMAR with guard regions.
// This is used for thread stacks, and for the thread area.
class GuardedPageBlock {
 public:
  constexpr GuardedPageBlock() = default;
  GuardedPageBlock(const GuardedPageBlock&) = delete;

  GuardedPageBlock(GuardedPageBlock&& other) noexcept
      : start_{std::exchange(other.start_, 0)},
        size_{std::exchange(other.size_, {})},
        vmar_{std::exchange(other.vmar_, {})} {}

  GuardedPageBlock& operator=(GuardedPageBlock&& other) noexcept {
    reset();
    start_ = std::exchange(other.start_, 0);
    size_ = std::exchange(other.size_, {});
    vmar_ = std::exchange(other.vmar_, {});
    return *this;
  }

  // Allocate a guarded block by consuming the next pages of the VMO.
  // The returned span does not include the guard regions.
  // The generic template is implemented inline below.
  template <typename T = std::byte>
  zx::result<std::span<T>> Allocate(zx::unowned_vmar allocate_from, AllocationVmo& vmo,
                                    PageRoundedSize data_size, PageRoundedSize guard_below,
                                    PageRoundedSize guard_above);

  void reset() {
    if (size_) {
      Unmap();
    }
  }

  [[nodiscard]] uintptr_t release() {
    vmar_ = {};
    size_ = {};
    return std::exchange(start_, 0);
  }

  ~GuardedPageBlock() { reset(); }

  size_t size_bytes() const { return size_.get(); }

  const zx::vmar& vmar() const { return *vmar_; }

 private:
  void Unmap();

  uintptr_t start_ = 0;
  PageRoundedSize size_;
  zx::unowned_vmar vmar_;
};

// The underlying implementation is out-of-line in this specialization.
template <>
zx::result<std::span<std::byte>> GuardedPageBlock::Allocate<std::byte>(
    zx::unowned_vmar allocate_from, AllocationVmo& vmo, PageRoundedSize data_size,
    PageRoundedSize guard_below, PageRoundedSize guard_above);

// The real implementation is the std::byte specialization, out of line.
// Others just convert the pointer type.
template <typename T>
inline zx::result<std::span<T>> GuardedPageBlock::Allocate(  //
    zx::unowned_vmar allocate_from, AllocationVmo& vmo, PageRoundedSize data_size,
    PageRoundedSize guard_below, PageRoundedSize guard_above) {
  zx::result result =
      Allocate<std::byte>(allocate_from->borrow(), vmo, data_size, guard_below, guard_above);
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::span{
      reinterpret_cast<T*>(result->data()),
      result->size_bytes() / sizeof(T),
  });
}

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_ZIRCON_VMAR_H_
