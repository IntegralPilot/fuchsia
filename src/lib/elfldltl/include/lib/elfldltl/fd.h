// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_FD_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_FD_H_

#include <lib/fit/result.h>
#include <unistd.h>

#include <cerrno>
#include <span>

#include "file.h"
#include "posix.h"

namespace elfldltl {

// elfldltl::FdFile is constructible from int file descriptor and meets the
// File API (see <lib/elfldltl/memory.h>) by calling pread.

namespace internal {

inline fit::result<PosixError> ReadFd(int fd, off_t offset, std::span<std::byte> buffer) {
  do {
    ssize_t n = pread(fd, buffer.data(), buffer.size(), offset);
    if (n < 0) {
      return fit::error{PosixError{errno}};
    }
    if (n == 0) {
      return fit::error{PosixError{}};  // This signifies EOF.
    }
    buffer = buffer.subspan(static_cast<size_t>(n));
    offset += n;
  } while (!buffer.empty());
  return fit::ok();
}

inline int MakeInvalidFd() { return -1; }

template <class Diagnostics>
using FdFileBase = File<Diagnostics, int, off_t, internal::ReadFd, internal::MakeInvalidFd>;

}  // namespace internal

template <class Diagnostics>
class FdFile : public internal::FdFileBase<Diagnostics> {
 public:
  using internal::FdFileBase<Diagnostics>::FdFileBase;

  FdFile(const FdFile&) noexcept = default;

  FdFile(FdFile&&) noexcept = default;

  FdFile& operator=(const FdFile&) noexcept = default;

  FdFile& operator=(FdFile&&) noexcept = default;

  int borrow() const { return this->get(); }
};

// Deduction guides.

template <class Diagnostics>
FdFile(int fd, Diagnostics& diagnostics) -> FdFile<Diagnostics>;

template <class Diagnostics>
FdFile(Diagnostics& diagnostics) -> FdFile<Diagnostics>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_FD_H_
