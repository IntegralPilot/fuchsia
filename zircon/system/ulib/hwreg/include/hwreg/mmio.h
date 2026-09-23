// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef HWREG_MMIO_H_
#define HWREG_MMIO_H_

#include <lib/mmio-ptr/mmio-ptr.h>

#include <cstdint>
#include <type_traits>

#include "internal.h"

namespace hwreg {

// This can be passed to ReadFrom and WriteTo methods. The RegisterAddr object holds an offset from
// an MMIO base address stored in this object.
//
// The template parameter defines a type meant to be used for MMIO operations (read/write).
// This implies that values are casted to and from |ForcedAccessType|. This affects register
// offsets as well, since offsets will be scaled with by |sizeof(ForcedAccessType)|.
//
// |ForcedAccessType = void| is a special case where unscaled and unconstrained MMIO operations are
// performed; that is no casting is performed between types and no scaling is applied to the
// offsets.

template <typename ForcedAccessType = void>
class RegisterMmioScaled {
 public:
  RegisterMmioScaled(volatile void* mmio) : mmio_(InitPtr(mmio)) {}

  // Write |val| to the |sizeof(IntType)| byte field located |offset| bytes from
  // |base()|.
  template <typename IntType>
  void Write(IntType val, uint32_t offset) {
    static_assert(sizeof(IntType) <= sizeof(IoType<IntType>));
    MmioWrite(static_cast<IoType<IntType>>(val), MmioPtr<IoType<IntType>>(offset));
  }

  // Read the value of the |sizeof(IntType)| byte field located |offset| bytes from
  // |base()|.
  template <typename IntType>
  IntType Read(uint32_t offset) {
    static_assert(sizeof(IntType) <= sizeof(IoType<IntType>));
    return static_cast<IntType>(MmioRead(MmioPtr<const IoType<IntType>>(offset)));
  }

  uintptr_t base() const { return reinterpret_cast<uintptr_t>(mmio_); }

 private:
  static_assert(std::is_void_v<ForcedAccessType> ||
                    hwreg::internal::IsSupportedInt<ForcedAccessType>::value,
                "Unsupported type.");

  template <typename IntType>
  using IoType = std::conditional_t<std::is_void_v<ForcedAccessType>, IntType, ForcedAccessType>;

  static constexpr uint32_t kScale =
      sizeof(std::conditional_t<std::is_void_v<ForcedAccessType>, char, ForcedAccessType>);

  MMIO_PTR volatile std::byte* InitPtr(volatile void* ptr) {
    uintptr_t addr = reinterpret_cast<uintptr_t>(ptr);
    return reinterpret_cast<MMIO_PTR volatile std::byte*>(addr);
  }

  template <typename U>
  MMIO_PTR volatile U* MmioPtr(uint32_t offset) {
    using PtrType = MMIO_PTR volatile U*;
    using IntType = std::remove_const_t<U>;
    static_assert(internal::IsSupportedInt<IntType>::value, "unsupported register access width");
    MMIO_PTR volatile std::byte* addr = mmio_ + (offset * kScale);
    return reinterpret_cast<PtrType>(addr);
  }

  MMIO_PTR volatile std::byte* mmio_ = nullptr;
};

using RegisterMmio = RegisterMmioScaled<>;
static_assert(std::is_copy_constructible_v<RegisterMmio>);

}  // namespace hwreg

#endif  // HWREG_MMIO_H_
