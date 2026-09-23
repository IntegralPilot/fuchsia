// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMO_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMO_H_

#include <lib/fit/result.h>
#include <lib/zx/vmo.h>
#include <zircon/errors.h>

#include <span>

#include "file.h"
#include "zircon.h"

namespace elfldltl {

// elfldltl::VmoFile is constructible from zx::vmo and meets the File API
// (see <lib/elfldltl/memory.h>) by calling zx_vmo_read.  The counterpart
// using zx::unowned_vmo is elfldltl::UnownedVmoFile.

namespace internal {

inline fit::result<ZirconError> ReadVmo(const zx::vmo& vmo, uint64_t offset,
                                        std::span<std::byte> buffer) {
  zx_status_t status = vmo.read(buffer.data(), offset, buffer.size());
  if (status == ZX_ERR_OUT_OF_RANGE) {
    return fit::error{ZirconError{}};  // This indicates EOF.
  }
  if (status != ZX_OK) {
    return fit::error{ZirconError{status}};
  }
  return fit::ok();
}

inline fit::result<ZirconError> ReadUnownedVmo(const zx::unowned_vmo& vmo, uint64_t offset,
                                               std::span<std::byte> buffer) {
  return ReadVmo(*vmo, offset, buffer);
}

template <typename T>
inline T DefaultConstruct() {
  return {};
}

}  // namespace internal

template <class Diagnostics>
using VmoFileBase = File<Diagnostics, zx::vmo, uint64_t, internal::ReadVmo>;

template <class Diagnostics>
using UnownedVmoFileBase = File<Diagnostics, zx::unowned_vmo, uint64_t, internal::ReadUnownedVmo>;

template <class Diagnostics>
class VmoFile : public VmoFileBase<Diagnostics> {
 public:
  using VmoFileBase<Diagnostics>::VmoFileBase;

  VmoFile(VmoFile&&) noexcept = default;

  VmoFile& operator=(VmoFile&&) noexcept = default;

  zx::unowned_vmo borrow() const { return this->get().borrow(); }
};

// Deduction guide.
template <class Diagnostics>
VmoFile(zx::vmo vmo, Diagnostics&& diagnostics) -> VmoFile<Diagnostics>;

template <class Diagnostics>
class UnownedVmoFile : public UnownedVmoFileBase<Diagnostics> {
 public:
  using UnownedVmoFileBase<Diagnostics>::UnownedVmoFileBase;

  UnownedVmoFile(const UnownedVmoFile&) noexcept = default;

  UnownedVmoFile(UnownedVmoFile&&) noexcept = default;

  UnownedVmoFile& operator=(const UnownedVmoFile&) noexcept = default;

  UnownedVmoFile& operator=(UnownedVmoFile&&) noexcept = default;

  zx::unowned_vmo borrow() const { return this->get()->borrow(); }
};

// Deduction guide.
template <class Diagnostics>
UnownedVmoFile(zx::unowned_vmo vmo, Diagnostics&& diagnostics) -> UnownedVmoFile<Diagnostics>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_VMO_H_
