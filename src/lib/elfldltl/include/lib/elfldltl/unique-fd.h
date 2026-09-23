// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_UNIQUE_FD_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_UNIQUE_FD_H_

#include <fbl/unique_fd.h>

#include "fd.h"

namespace elfldltl {

// elfldltl::UniqueFdFile is constructible from fbl::unique_fd and meets the
// File API (see <lib/elfldltl/memory.h>) by calling pread.
//
// See <lib/elfldltl/fd.h>, on which this is based.

namespace internal {

inline fit::result<PosixError> ReadUniqueFd(const fbl::unique_fd& fd, off_t offset,
                                            std::span<std::byte> buffer) {
  return ReadFd(fd.get(), offset, buffer);
}

template <class Diagnostics>
using UniqueFdFileBase = File<Diagnostics, fbl::unique_fd, off_t, internal::ReadUniqueFd>;

}  // namespace internal

template <class Diagnostics>
class UniqueFdFile : public internal::UniqueFdFileBase<Diagnostics> {
 public:
  using Base = internal::UniqueFdFileBase<Diagnostics>;

  using Base::Base;

  UniqueFdFile(UniqueFdFile&&) noexcept = default;

  UniqueFdFile& operator=(UniqueFdFile&&) noexcept = default;

  int get() const { return Base::get().get(); }

  int borrow() const { return get(); }
};

// Deduction guides.

template <class Diagnostics>
UniqueFdFile(fbl::unique_fd fd, Diagnostics& diagnostics) -> UniqueFdFile<Diagnostics>;

template <class Diagnostics>
UniqueFdFile(Diagnostics& diagnostics) -> UniqueFdFile<Diagnostics>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_UNIQUE_FD_H_
