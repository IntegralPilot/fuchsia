// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ABI_PTR_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ABI_PTR_H_

#include <cstdint>
#include <type_traits>

#include "layout.h"

namespace elfldltl {

// elfldltl::AbiPtr<T> is like a T* but can support a stable and cross-friendly
// ABI.  It can be used to store pointers that use a different byte order,
// address size, or address space, than the current process.  Its main purpose
// is for conversions from pointers in the local address space into pointers in
// a different address space and possibly different format.  The conversions
// are handled elsewhere (TODO(mcgrathr): not yet implemented).
//
// The byte order and address are represented by an elfldltl::Elf<...> type in
// the Elf template parameter; it defaults to the native elfldltl::Elf<>.  The
// type in the Traits template parameter determines whether these are actually
// used in how the pointer is stored; it defaults to LocalAbiTraits, which just
// stores a normal native pointer regardless of the Elf size and encoding.
//
// elfldltl::AbiPtr<T> should be used instead of normal T* in all data
// structures that will exist in one process address space but be populated by
// code running in a different address space.  Such data structures should be
// templated on Elf and AbiTraits classes that are propagated to the
// elfldltl::AbiPtr<T, ...> instantiations they use for pointer-type members.
//
// elfldltl::AbiPtr<T> always supports all the arithmetic operators that real
// T* pointers do.  The Traits type might or might not allow converting the
// stored pointer to a real pointer in the local address space.  If it does,
// then elfldltl::AbiPtr<T> can be used like a smart-pointer type with `get()`
// and `*` and `->` operators and can be constructed from real T* pointers.

// Each Traits class is defined just once for all data types and ELF layouts,
// with member types and static members that are all templated on Elf and T
// themselves individually.  The AbiTraitsApi concept can only check that the
// Traits class meets the API contract for a given Elf and T instantiation, but
// each class is expected to work with any combination of types.

// This concept is used as part of the AbiTraitsApi concept.
template <typename StorageType, typename T, typename size_type>
concept AbiPtrTraitsStorageType =
    std::same_as<StorageType, T*> || requires(StorageType storage, size_type unsigned_integer,
                                              std::make_signed_t<size_type> signed_integer) {
      { storage + unsigned_integer } -> std::convertible_to<StorageType>;
      { storage + signed_integer } -> std::convertible_to<StorageType>;
      { storage - unsigned_integer } -> std::convertible_to<StorageType>;
      { storage - signed_integer } -> std::convertible_to<StorageType>;
      { storage - storage } -> std::integral;
    };

// Any Traits type used with AbiPtr must meet the AbiTraitsApi requirements.
template <class Traits, typename T, class Elf>
concept AbiPtrTraitsApi =
    ElfApi<Elf> && requires(Elf::Addr addr, Traits::template StorageType<Elf, T> storage) {
      // This must be defined to some type that admits + and - operators with
      // integer types, and operator- with itself.  Pointer types can satisfy
      // this even while incomplete, though they'll have to be complete by the
      // time the AbiPtr arithmetic methods are used.
      typename Traits::template StorageType<Elf, T>;
      {
        typename Traits::template StorageType<Elf, T>{}
      } -> AbiPtrTraitsStorageType<T, typename Elf::size_type>;

      // The kScale<T> template static constexpr variable must be defined to a
      // factor by which an integer should be scaled up before adding or
      // subtracting it to a StorageType<..., T> value, and by which the value
      // of subtracting two such values should be scaled down.
      //
      // This is not imposed as a formal requirement because that requires
      // instantiating kScale<T> as part of the instantiation of AbiPtr<T>, and
      // that might require T to be a complete type (e.g. if sizeof(T) is used
      // in kScale<T> as in RemoteAbiTraits).  But that would make it
      // impossible to declare self-referential data types using AbiPtr.  {
      // std::integral_constant<decltype(Traits::template kScale<T>),
      // Traits::template kScale<T>>::value + 0 } -> std::unsigned_integral;

      // This must be defined to accept any StorageType<...> argument and yield
      // the address in the target address space as some unsigned integer type.
      { Traits::GetAddress(storage) } -> std::unsigned_integral;

      // This must be defined to accept any StorageType<...> argument and yield
      // a type that admits operator<=> for comparison of two pointers it's
      // valid to compare by C rules.
      { Traits::ComparisonValue(storage) } -> std::totally_ordered;

      // This must be defined to accept Elf::Addr (or Elf::size_type, since
      // they are mutually convertible).
      { Traits::template FromAddress<Elf, T>(addr) } -> std::convertible_to<decltype(storage)>;
    };

// If an AbiTraits class offers the Make and Get methods to convert directly to
// a local pointer, it qualifies as a LocalAbiTraitsApi class.  In this case,
// AbiPtr will support all the smart-pointer methods and operators and be
// convertible to and from normal pointers (T*).
template <class Traits, typename T, class Elf>
concept AbiPtrLocalTraitsApi =
    AbiPtrTraitsApi<Traits, T, Elf> &&
    requires(T* ptr, Traits::template StorageType<Elf, T> storage) {
      {
        Traits::template Make<Elf, T>(ptr)
      } -> std::convertible_to<typename Traits::template StorageType<Elf, T>>;

      { Traits::template Get<Elf, T>(storage) } -> std::convertible_to<T*>;
    };

template <typename T, ElfApi Elf = elfldltl::Elf<>, AbiPtrTraitsApi<T, Elf> Traits = LocalAbiTraits>
struct AbiPtr {
 public:
  using value_type = T;

  // This is the type used to represent an address in the target address space.
  using Addr = Elf::Addr;

  // No matter how the pointer is represented, sizes of objects or arrays it
  // points to must fit into size_type.
  using size_type = Elf::size_type;

  // The Traits type determines the underlying type stored.
  using StorageType = Traits::template StorageType<Elf, T>;

  // This provides a convenient shorthand for checking if AbiPtr is just
  // interchangeable with normal pointers.
  static constexpr bool kLocal = AbiPtrLocalTraitsApi<Traits, T, Elf>;

  constexpr AbiPtr() = default;

  constexpr AbiPtr(const AbiPtr&) = default;

  // If Traits supports it, AbiPtr can be constructed from a normal pointer.
  constexpr explicit AbiPtr(value_type* ptr)
    requires kLocal
      : storage_(Traits::template Make<Elf, value_type>(ptr)) {}

  constexpr AbiPtr& operator=(const AbiPtr&) = default;

  constexpr AbiPtr& operator=(value_type* ptr)
    requires kLocal
  {
    *this = AbiPtr{ptr};
    return *this;
  }

  // AbiPtr<T> is convertible to AbiPtr<U> exactly like T* to U*.
  template <typename U>
    requires std::is_convertible_v<T*, U*>
  constexpr explicit(!std::convertible_to<T*, U*>) operator AbiPtr<U, Elf, Traits>() const {
    return Reinterpret<U>();
  }

  static constexpr AbiPtr FromAddress(Addr address) {
    return AbiPtr{Traits::template FromAddress<Elf, T>(address), std::in_place};
  }

  // Like `reinterpret_cast<Other*>(this->get())`.
  template <typename Other>
  constexpr AbiPtr<Other, Elf, Traits> Reinterpret() const {
    return AbiPtr<Other, Elf, Traits>::FromAddress(address());
  }

  constexpr explicit operator bool() const { return *this != AbiPtr{}; }

  constexpr bool operator==(const AbiPtr& other) const {
    return ComparisonValue() == other.ComparisonValue();
  }

  constexpr auto operator<=>(const AbiPtr& other) const {
    return ComparisonValue() <=> other.ComparisonValue();
  }

  constexpr AbiPtr operator+(size_type n) const {
    return AbiPtr{storage_ + (n * kScale), std::in_place};
  }

  constexpr AbiPtr operator-(size_type n) const {
    return AbiPtr{storage_ - (n * kScale), std::in_place};
  }

  constexpr size_type operator-(const AbiPtr& other) const {
    return static_cast<size_type>(storage_ - other.storage_) / kScale;
  }

  constexpr AbiPtr& operator+=(size_type n) {
    *this = *this + n;
    return *this;
  }

  constexpr AbiPtr& operator-=(size_type n) {
    *this = *this - n;
    return *this;
  }

  // This just returns the address in the target address space.
  constexpr size_type address() const {
    return static_cast<size_type>(Traits::GetAddress(storage_));
  }

  // Dereferencing methods are only available if enabled by the Traits type.

  constexpr T* get() const
    requires kLocal
  {
    return Traits::template Get<Elf, T>(storage_);
  }

  constexpr T* operator->() const
    requires kLocal
  {
    return get();
  }

  constexpr T& operator*() const
    requires kLocal
  {
    return *get();
  }

 private:
  static constexpr size_type kScale = Traits::template kScale<T>;

  constexpr explicit AbiPtr(StorageType storage, std::in_place_t in_place) : storage_{storage} {}

  constexpr auto ComparisonValue() const { return Traits::ComparisonValue(storage_); }

  constexpr size_type Scale(size_type n) { return n * kScale; }

  StorageType storage_{};
};

// Deduction guide.
template <typename T>
AbiPtr(T*) -> AbiPtr<T>;

// This is the default Traits type for AbiPtr and things that use it.
// It meets both AbiTraitsApi and LocalAbiTraitsApi for any Elf and T.
struct LocalAbiTraits {
  template <ElfApi Elf, typename T>
  using StorageType = T*;

  template <typename T>
  static constexpr uint32_t kScale = 1;

  static uintptr_t GetAddress(const void* ptr) { return reinterpret_cast<uintptr_t>(ptr); }

  template <typename T>
  static constexpr auto ComparisonValue(T* ptr) {
    return ptr;
  }

  template <ElfApi Elf, typename T>
  static StorageType<Elf, T> FromAddress(uintptr_t address) {
    return reinterpret_cast<T*>(address);
  }

  // LocalAbiTraitsApi methods.

  template <ElfApi Elf, typename T>
  static constexpr StorageType<Elf, T> Make(T* ptr) {
    return ptr;
  }

  template <ElfApi Elf, typename T>
  static constexpr T* Get(StorageType<Elf, T> ptr) {
    return ptr;
  }
};
static_assert(AbiPtrLocalTraitsApi<LocalAbiTraits, Elf<>::Addr, Elf<>>);

// This is a baseline Traits type for storing pointers in a different address
// space and pointer encoding.  An AbiPtr<..., RemoteAbiTraits> type doesn't
// support the get() method and the pointer-like operators.
//
// elfldltl::AbiPtr<T, Elf, elfldltl::RemoteAbiTraits> has in every context the
// same layout that elfldltl::AbiPtr<T> does wherever elfldltl::Elf<> is Elf.
//
// This meets AbiTraitsApi for any Elf and T, but not LocalAbiTraitsApi.
struct RemoteAbiTraits {
  template <ElfApi Elf, typename T>
  using StorageType = Elf::Addr;

  template <typename T>
  static constexpr uint32_t kScale = sizeof(T);

  static constexpr auto GetAddress(auto ptr) { return ptr(); }

  static constexpr auto ComparisonValue(auto ptr) { return ptr(); }

  template <ElfApi Elf, typename T>
  static constexpr StorageType<Elf, T> FromAddress(Elf::Addr address) {
    return address;
  }
};
static_assert(AbiPtrTraitsApi<RemoteAbiTraits, Elf<>::Addr, Elf<>>);

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_ABI_PTR_H_
