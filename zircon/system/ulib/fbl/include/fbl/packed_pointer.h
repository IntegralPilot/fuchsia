// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FBL_PACKED_POINTER_H_
#define FBL_PACKED_POINTER_H_

#include <zircon/assert.h>
#include <zircon/types.h>

#include <bit>
#include <cstddef>
#include <type_traits>

namespace fbl {

namespace internal {
template <typename T, size_t kDataBits, bool kCheckAlignment>
struct PackedPointerAlignmentValidator {
  static constexpr bool value = true;
};

template <typename T, size_t kDataBits>
struct PackedPointerAlignmentValidator<T, kDataBits, true> {
  static constexpr bool value = (alignof(T) >= (1ul << kDataBits));
};
}  // namespace internal

// PackedPointer<T, kDataBits, kCheckAlignment> is a pointer wrapper that allows
// storing a small amount of data in the alignment bits of the pointer.
//
// The number of bits available for packing (kDataBits) must be less than or
// equal to the number of trailing zero bits in the alignment of T. For example,
// if alignof(T) == 8, then kDataBits can be up to 3.
//
// PackedPointer provides a safety check at compile time to ensure that the
// requested number of data bits is compatible with the alignment of T. It also
// provides runtime assertions (in debug builds) to ensure that the pointer
// passed to it is correctly aligned and that the data fits within the specified
// number of bits.
//
// In some cases, such as when T is an incomplete type (e.g. when PackedPointer
// is used as a member of T), the compile-time alignment check cannot be
// performed. In these cases, kCheckAlignment can be set to false to disable
// the check.
//
// Example usage:
//
//   struct alignas(8) MyStruct { ... };
//   PackedPointer<MyStruct, 3> ptr(my_struct_instance, 0x5);
//
//   MyStruct* p = ptr.ptr();  // returns my_struct_instance
//   uintptr_t d = ptr.data(); // returns 0x5
//
template <typename T, size_t kDataBits, bool kCheckAlignment = true>
class PackedPointer {
 public:
  static_assert(kDataBits > 0, "PackedPointer requires at least one data bit.");
  static_assert(kDataBits < (sizeof(uintptr_t) * 8), "Too many data bits requested.");
  static_assert(internal::PackedPointerAlignmentValidator<T, kDataBits, kCheckAlignment>::value,
                "T has insufficient alignment for the requested number of data bits.");

  static constexpr uintptr_t kDataMask = (1ul << kDataBits) - 1;
  static constexpr uintptr_t kPtrMask = ~kDataMask;

  constexpr PackedPointer() = default;
  constexpr PackedPointer(std::nullptr_t) : value_(0) {}

  explicit PackedPointer(T* ptr) : value_(std::bit_cast<uintptr_t>(ptr)) {
    // Here (and elsewhere) we check that the pointer provided is correctly aligned, even if the
    // kCheckAlignment should have guaranteed this. The motivation is that although a `T*` with
    // alignment bits set would be an invalid pointer, this check serves to guard against callers
    // misusing the interface and 'pre packing' their data into the pointer, instead of using the
    // separate constructors and setters for manipulating pointer and data independently.
    ZX_DEBUG_ASSERT_MSG((value_ & kDataMask) == 0,
                        "Pointer %p is not aligned to at least %zu bytes", ptr,
                        size_t{1} << kDataBits);
  }

  PackedPointer(T* ptr, uintptr_t data) : value_(std::bit_cast<uintptr_t>(ptr) | data) {
    ZX_DEBUG_ASSERT_MSG((std::bit_cast<uintptr_t>(ptr) & kDataMask) == 0,
                        "Pointer %p is not aligned to at least %zu bytes", ptr,
                        size_t{1} << kDataBits);
    ZX_DEBUG_ASSERT_MSG((data & kPtrMask) == 0, "Data %zu exceeds %zu bits", data, kDataBits);
  }

  PackedPointer(std::nullptr_t, uintptr_t data) : value_(data) {
    ZX_DEBUG_ASSERT_MSG((data & kPtrMask) == 0, "Data %zu exceeds %zu bits", data, kDataBits);
  }

  T* ptr() const { return std::bit_cast<T*>(value_ & kPtrMask); }
  uintptr_t data() const { return value_ & kDataMask; }

  void set_ptr(T* ptr) {
    uintptr_t raw_ptr = std::bit_cast<uintptr_t>(ptr);
    ZX_DEBUG_ASSERT_MSG((raw_ptr & kDataMask) == 0,
                        "Pointer %p is not aligned to at least %zu bytes", ptr,
                        size_t{1} << kDataBits);
    value_ = (value_ & kDataMask) | raw_ptr;
  }

  void set_data(uintptr_t data) {
    ZX_DEBUG_ASSERT_MSG((data & kPtrMask) == 0, "Data %zu exceeds %zu bits", data, kDataBits);
    value_ = (value_ & kPtrMask) | data;
  }

  void reset() { value_ = 0; }

  // Pointer semantics
  T& operator*() const { return *ptr(); }
  T* operator->() const { return ptr(); }
  explicit operator bool() const { return ptr() != nullptr; }

  // Comparison operators
  bool operator==(const PackedPointer& other) const { return value_ == other.value_; }
  bool operator!=(const PackedPointer& other) const { return value_ != other.value_; }
  bool operator==(std::nullptr_t) const { return ptr() == nullptr; }
  bool operator!=(std::nullptr_t) const { return ptr() != nullptr; }

 private:
  uintptr_t value_ = 0;
};

}  // namespace fbl

#endif  // FBL_PACKED_POINTER_H_
