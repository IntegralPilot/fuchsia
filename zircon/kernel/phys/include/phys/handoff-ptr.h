// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_HANDOFF_PTR_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_HANDOFF_PTR_H_

// Note: we refrain from using the ktl namespace as <phys/handoff.h> is
// expected to be compiled in the userboot toolchain.

#include <assert.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <span>
#include <string_view>
#include <utility>

struct PhysHandoff;
extern PhysHandoff* gPhysHandoff;

// PhysHandoffPtr provides a "smart pointer" style API for pointers handed off
// from physboot to the kernel proper.  A handoff pointer is only ever created
// in physboot by the HandoffPrep class.  It's only ever dereferenced (or
// converted into a raw pointer) in the kernel proper.  When converted to a
// pointer, it's only ever converted to a pointer to const.
//
// Lifetime issues for handoff data are complex.  PhysHandoffPtr is always
// treated as a traditional "owning" smart pointer and is a move-only type.
// Ordinarily, handoff pointer objects will be left in place and only have raw
// pointers extracted from them for later use.

// PhysHandoffPtr has no destructor and the "owning" pointer dying doesn't have
// any direct effect.  The lifetime of all handoff pointers is actually grouped
// in three buckets:
//
//  * Permanent handoff data will be accessible in the kernel's virtual address
//    space permanently.  This data resides on pages that the PMM has been told
//    are owned by kernel mappings.
//
//  * Pointers into the kernel's own load image.  From the kernel's perspective
//    these are just more permanent pointers.  However, in physboot they are
//    created by translating from existing locations in the kernel ELF file
//    that was mapped at a virtual address, whereas permanent handoff data is
//    in physical pages specifically allocated for that purpose by physboot.
//
//  * Temporary handoff data must be consumed only during the handoff phase,
//    which ends once EndHandoff() is called.  This data resides on pages that
//    the PMM may be told to reuse after handoff.
//
enum class PhysHandoffPtrLifetime {
  // In normal RAM handed off as mempool::Type::kTemporaryPhysHandoff.  These
  // are normal pages (mapped in the physmap as well as this virtual mapping).
  // But the kernel will reclaim them in EndHandoff, so they are referenced
  // only in PhysHandoff and not in BootConstants.
  kTemporary,

  // In normal RAM handed off as mempool::Type::kPermanentPhysHandoff.  These
  // are normal pages (mapped in the physmap as well as this virtual mapping),
  // but wired forever in the kernel.  These pointers can live anywhere.
  kPermanent,

  // In the kernel's own ELF load image.  This is not quite normal RAM!  Its
  // pages are handed off as mempool::Type::kKernel and managed by the VM
  // system (and wired forever).  But it is kept out of the physmap, so in the
  // kernel proper it's only directly accessible through protected mappings.
  kKernelImage,

  // This represents a mapping to physical pages (whether normal or MMIO) that
  // is outside the VM system's management of physical pages (PMM).  These
  // mappings stay wired forever in the kernel; the pointers can live anywhere.
  kPhysical,
};

// Forward declaration; see below.
template <typename T, PhysHandoffPtrLifetime Lifetime>
class PhysHandoffSpan;

// Forward declaration; see below.
template <typename T, PhysHandoffPtrLifetime Lifetime>
class PhysHandoffSpan;

template <typename T>
consteval bool PhysHandoffPtrValidType(PhysHandoffPtrLifetime Lifetime) {
  switch (Lifetime) {
    case PhysHandoffPtrLifetime::kTemporary:
    case PhysHandoffPtrLifetime::kPermanent:
      // Handoff "heap" pointers are always pointers to const.
      return std::is_const_v<T>;

    case PhysHandoffPtrLifetime::kKernelImage:
      return true;

    case PhysHandoffPtrLifetime::kPhysical:
      // Physical pointers are never const: the virtual mappings are always
      // writable, so they can be dereferenced freely in phys as in the kernel proper.
      return !std::is_const_v<T>;
  }
}

// Most handoff pointers can only be dereferenced in the kernel proper.
constexpr bool kPhysHandoffPtrCanDeref =
#ifdef HANDOFF_PTR_DEREF
    HANDOFF_PTR_DEREF
#elif defined(_KERNEL)
    true
#else
    false
#endif
    ;

template <typename T, PhysHandoffPtrLifetime Lifetime>
  requires(PhysHandoffPtrValidType<T>(Lifetime))
class PhysHandoffPtr {
 public:
  using value_type = T;

  static constexpr bool kCanDeref =
      kPhysHandoffPtrCanDeref || Lifetime == PhysHandoffPtrLifetime::kPhysical;

  // Default-constructible, movable but not copyable (use .get() instead).
  constexpr PhysHandoffPtr() = default;
  constexpr PhysHandoffPtr(PhysHandoffPtr&& other) noexcept : ptr_(std::exchange(other.ptr_, {})) {}

  constexpr PhysHandoffPtr(const PhysHandoffPtr&)
    requires(kCanDeref)  // It's copyable when kCanDeref, move-only otherwise.
  = default;

  // In the kernel proper, pointers that are definitely into the image itself
  // can be initialized as constinit.
  consteval explicit PhysHandoffPtr(T& ref)
    requires(kCanDeref && Lifetime == PhysHandoffPtrLifetime::kKernelImage)
      : ptr_{&ref} {}

  constexpr PhysHandoffPtr& operator=(PhysHandoffPtr&& other) noexcept {
    ptr_ = std::exchange(other.ptr_, {});
    return *this;
  }

  constexpr PhysHandoffPtr& operator=(const PhysHandoffPtr&)
    requires(kCanDeref)  // It's copyable when kCanDeref, move-only otherwise.
  = default;

  constexpr auto operator<=>(const PhysHandoffPtr& other) const = default;

  explicit constexpr operator bool() const { return ptr_; }

  constexpr value_type* get() const
    requires(kCanDeref)
  {
    if constexpr (Lifetime == PhysHandoffPtrLifetime::kTemporary) {
      ZX_ASSERT_MSG(gPhysHandoff, "Pointer no longer valid; phys hand-off has already ended!");
    }
    return ptr_;
  }

  // This is allowed for debugging purposes in physboot.
  constexpr value_type* force_get() const { return ptr_; }

  [[nodiscard]] value_type* release()
    requires(kCanDeref)
  {
    return std::exchange(ptr_, {});
  }

  value_type& operator*() const
    requires(kCanDeref)
  {
    return *get();
  }

  value_type* operator->() const
    requires(kCanDeref)
  {
    return get();
  }

  // The equivalent of reinterpret_cast can be done even when !kCanDeref.

  uintptr_t address() const {  // as if reinterpret_cast<uintptr_t>(get())
    return reinterpret_cast<uintptr_t>(ptr_);
  }

  template <typename Other>  // as if reinterpret_cast<Other*>(get())
    requires(std::is_const_v<Other> == std::is_const_v<T>)
  PhysHandoffPtr<Other, Lifetime> Reinterpret() const {
    using OtherPtr = PhysHandoffPtr<Other, Lifetime>;
    OtherPtr other;
    other.ptr_ = reinterpret_cast<OtherPtr::value_type*>(ptr_);
    return other;
  }

  template <typename Other>
    requires std::same_as<std::remove_const_t<Other>, std::remove_const_t<T>>
  PhysHandoffPtr<Other, Lifetime> ConstCast() const {
    using OtherPtr = PhysHandoffPtr<Other, Lifetime>;
    OtherPtr other;
    other.ptr_ = const_cast<OtherPtr::value_type*>(ptr_);
    return other;
  }

 private:
  friend class HandoffPrep;
  friend class PhysHandoffSpan<T, Lifetime>;
  template <typename Other, PhysHandoffPtrLifetime OtherLifetime>
    requires(PhysHandoffPtrValidType<Other>(OtherLifetime))
  friend class PhysHandoffPtr;

  value_type* ptr_ = nullptr;
};

// PhysHandoffSpan<T> is to std::span<T> as PhysHandoffPtr<T> is to T*.  It has
// get() and release() methods that return std::span<T>.
template <typename T, PhysHandoffPtrLifetime Lifetime>
class PhysHandoffSpan {
 public:
  using Ptr = PhysHandoffPtr<T, Lifetime>;
  using value_type = Ptr::value_type;

  constexpr PhysHandoffSpan() = default;
  constexpr PhysHandoffSpan(const PhysHandoffSpan&) = default;
  constexpr PhysHandoffSpan(PhysHandoffSpan&&) noexcept = default;

  constexpr PhysHandoffSpan(Ptr ptr, size_t size) : ptr_(std::move(ptr)), size_(size) {}

  constexpr PhysHandoffSpan& operator=(const PhysHandoffSpan&) noexcept = default;
  constexpr PhysHandoffSpan& operator=(PhysHandoffSpan&&) noexcept = default;

  constexpr auto operator<=>(const PhysHandoffSpan& other) const = default;

  constexpr size_t size() const { return size_; }

  constexpr size_t size_bytes() const { return size_ * sizeof(value_type); }

  constexpr bool empty() const { return size() == 0; }

  constexpr value_type* data() const
    requires(Ptr::kCanDeref)
  {
    return ptr_.get();
  }

  constexpr std::span<value_type> get() const
    requires(Ptr::kCanDeref)
  {
    return force_get();
  }

  // This is allowed for debugging purposes in physboot.
  constexpr std::span<value_type> force_get() const { return {ptr_.force_get(), size_}; }

  [[nodiscard]] constexpr std::span<value_type> release()
    requires(Ptr::kCanDeref)
  {
    return {ptr_.release(), size_};
  }

  constexpr uintptr_t address() const { return ptr_.address(); }

  constexpr PhysHandoffSpan subspan(size_t offset, size_t count) const {
    PhysHandoffSpan result;
    assert(offset <= size_);
    result.ptr_.ptr_ = ptr_.ptr_ + offset;
    result.size_ = count;
    return result;
  }

 private:
  friend class HandoffPrep;

  Ptr ptr_;
  size_t size_ = 0;
};

// PhysHandoffString is stored just the same as PhysHandoffSpan<const char>,
// but its get() and release() methods yield std::string_view.
template <PhysHandoffPtrLifetime Lifetime>
class PhysHandoffString : public PhysHandoffSpan<const char, Lifetime> {
 public:
  using Base = PhysHandoffSpan<const char, Lifetime>;

  constexpr PhysHandoffString() = default;
  constexpr PhysHandoffString(PhysHandoffString&&) noexcept = default;
  constexpr PhysHandoffString& operator=(PhysHandoffString&&) noexcept = default;

  constexpr const char* data() const
    requires(Base::Ptr::kCanDeref)
  {
    return Base::data();
  }

  PhysHandoffString substr(size_t offset, size_t count) const {
    PhysHandoffString result;
    result.Base::operator=(Base::subspan(offset, count));
    return result;
  }

  constexpr std::string_view get() const
    requires(Base::Ptr::kCanDeref)
  {
    return {data(), Base::size()};
  }

  [[nodiscard]] constexpr std::string_view release()
    requires(Base::Ptr::kCanDeref)
  {
    std::span str = Base::release();
    return {str.data(), str.size()};
  }
};

// Convenience aliases used in the PhysHandoff declaration.

template <typename T>
using PhysHandoffTemporaryPtr = PhysHandoffPtr<const T, PhysHandoffPtrLifetime::kTemporary>;

template <typename T>
using PhysHandoffTemporarySpan = PhysHandoffSpan<const T, PhysHandoffPtrLifetime::kTemporary>;

using PhysHandoffTemporaryString = PhysHandoffString<PhysHandoffPtrLifetime::kTemporary>;

template <typename T>
using PhysHandoffPermanentPtr = PhysHandoffPtr<const T, PhysHandoffPtrLifetime::kPermanent>;

template <typename T>
using PhysHandoffPermanentSpan = PhysHandoffSpan<const T, PhysHandoffPtrLifetime::kPermanent>;

using PhysHandoffPermanentString = PhysHandoffString<PhysHandoffPtrLifetime::kPermanent>;

template <typename T>
using PhysHandoffKernelImagePtr = PhysHandoffPtr<T, PhysHandoffPtrLifetime::kKernelImage>;

template <typename T>
using PhysHandoffKernelImageSpan = PhysHandoffSpan<T, PhysHandoffPtrLifetime::kKernelImage>;

using PhysHandoffKernelImageString = PhysHandoffString<PhysHandoffPtrLifetime::kKernelImage>;

template <typename T>
using PhysHandoffPhysicalPtr = PhysHandoffPtr<T, PhysHandoffPtrLifetime::kPhysical>;

template <typename T>
using PhysHandoffPhysicalSpan = PhysHandoffSpan<T, PhysHandoffPtrLifetime::kPhysical>;

// A mapped MMIO region is a kPhysical span that also carries its paddr.
class MappedMmioRange : public PhysHandoffPhysicalSpan<volatile std::byte> {
 public:
  constexpr MappedMmioRange() = default;

  constexpr MappedMmioRange(const MappedMmioRange&) = default;

  constexpr MappedMmioRange& operator=(const MappedMmioRange&) = default;

  // Sort only by paddr.
  constexpr auto operator<=>(const MappedMmioRange& other) const { return paddr_ <=> other.paddr_; }

  constexpr uint64_t paddr() const { return paddr_; }

  constexpr uint64_t paddr_end() const { return paddr_ + size_bytes(); }

  constexpr bool contains_paddr(uint64_t paddr) const {
    return paddr >= paddr_ && paddr - paddr_ < size_bytes();
  }

  uintptr_t vaddr() const { return reinterpret_cast<uintptr_t>(data()); }

  uintptr_t vaddr_end() const { return vaddr() + size_bytes(); }

  bool contains_vaddr(uintptr_t addr) const {
    return addr >= vaddr() && addr - vaddr() < size_bytes();
  }

 private:
  friend class HandoffPrep;

  using Base = PhysHandoffPhysicalSpan<volatile std::byte>;

  using Base::operator=;

  uint64_t paddr_ = 0;
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_HANDOFF_PTR_H_
