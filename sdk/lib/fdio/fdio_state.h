// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FDIO_FDIO_STATE_H_
#define LIB_FDIO_FDIO_STATE_H_

#include <lib/fdio/limits.h>
#include <lib/fdio/namespace.h>
#include <sys/types.h>  // mode_t

#include <array>
#include <mutex>

#include "sdk/lib/fdio/cleanpath.h"
#include "sdk/lib/fdio/fdio_slot.h"

struct fdio_state_t {
  std::optional<int> bind_to_fd_locked(const fbl::RefPtr<fdio>& io) __TA_REQUIRES(lock);
  std::optional<int> bind_to_fd(const fbl::RefPtr<fdio>& io) __TA_EXCLUDES(lock);
  fbl::RefPtr<fdio> fd_to_io_locked(int fd) __TA_REQUIRES(lock);
  fbl::RefPtr<fdio> fd_to_io(int fd) __TA_EXCLUDES(lock);
  fbl::RefPtr<fdio> unbind_from_fd_locked(int fd) __TA_REQUIRES(lock);
  fbl::RefPtr<fdio> unbind_from_fd(int fd) __TA_EXCLUDES(lock);

  // TODO(tamird): make these private and make this a class.
  std::mutex lock;
  std::mutex cwd_lock __TA_ACQUIRED_BEFORE(lock);
  mode_t umask __TA_GUARDED(lock);
  fdio_slot root __TA_GUARDED(lock);
  fdio_slot cwd __TA_GUARDED(lock);
  std::array<fdio_slot, FDIO_MAX_FD> fdtab __TA_GUARDED(lock);
  fdio_ns_t* ns __TA_GUARDED(lock);
  fdio_internal::PathBuffer cwd_path __TA_GUARDED(cwd_lock);
};

fdio_state_t& fdio_global_state();

#endif  // LIB_FDIO_FDIO_STATE_H_
