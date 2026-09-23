// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/fdio/fdio_state.h"

#include <array>
#include <mutex>
#include <optional>

namespace {

fdio_slot* slot_locked(std::array<fdio_slot, FDIO_MAX_FD>& fdtab, int fd) {
  if ((fd < 0) || (fd >= FDIO_MAX_FD)) {
    return nullptr;
  }
  return &fdtab[fd];
}

}  // namespace

fdio_state_t& fdio_global_state() {
  // The C++20 `constinit` specifier ensures that no runtime initialization is needed. The
  // `clang::no_destroy` attribute ensures that no automatic runtime finalization is done, since
  // it's difficult to control its ordering with respect to other finalization routines; instead we
  // explicitly invoke the destructor in `__libc_extensions_fini` which happens safely after all
  // other finalization code.
  [[clang::no_destroy]] static constinit fdio_state_t state{
      .cwd_path = fdio_internal::PathBuffer('/'),
  };
  return state;
}

std::optional<int> fdio_state_t::bind_to_fd(const fbl::RefPtr<fdio>& io) {
  std::lock_guard guard(lock);
  return bind_to_fd_locked(io);
}

std::optional<int> fdio_state_t::bind_to_fd_locked(const fbl::RefPtr<fdio>& io) {
  for (size_t fd = 0; fd < fdtab.size(); ++fd) {
    if (fdtab[fd].try_set(io)) {
      return fd;
    }
  }
  return std::nullopt;
}

fbl::RefPtr<fdio> fdio_state_t::fd_to_io(int fd) {
  std::lock_guard guard(lock);
  return fd_to_io_locked(fd);
}

fbl::RefPtr<fdio> fdio_state_t::fd_to_io_locked(int fd) {
  fdio_slot* slot = slot_locked(fdtab, fd);
  if (slot == nullptr) {
    return nullptr;
  }
  return slot->get();
}

fbl::RefPtr<fdio> fdio_state_t::unbind_from_fd(int fd) {
  std::lock_guard guard(lock);
  return unbind_from_fd_locked(fd);
}

fbl::RefPtr<fdio> fdio_state_t::unbind_from_fd_locked(int fd) {
  fdio_slot* slot = slot_locked(fdtab, fd);
  if (slot == nullptr) {
    return nullptr;
  }
  return slot->release();
}
