// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/unique-backtrace.h>

#include <cassert>

#include "thread-list.h"
#include "thread-storage.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

zx::result<> ThreadDetach(Thread& thread) {
  // Try to claim the join slot on this thread on behalf of the thread.
  auto old_state = thread.JoinOrDetachLifecycle(Thread::Lifecycle::DETACHED);
  if (!old_state) {  // Was joinable, is now detached.
    return zx::ok();
  }

  // Otherwise, the thread wasn't joinable for some reason.
  switch (*old_state) {
    case Thread::Lifecycle::DETACHED:
    case Thread::Lifecycle::JOINED:
      // The thread isn't joinable.  It was already joined or detached.
      return zx::error{ZX_ERR_INVALID_ARGS};

    case Thread::Lifecycle::EXITING:
      // Since it is undefined behavior to call ThreadDetach on a thread that
      // has already been detached or joined, we assume the state prior to
      // EXITING was JOINABLE.  However, since the thread is already shutting
      // down, it is too late to tell it to clean itself up.  Since the
      // thread is still running, we cannot just return ZX_ERR_BAD_STATE,
      // which would suggest we couldn't detach and the thread has already
      // finished running.  Instead, we call ThreadJoin, which will return
      // soon due to the thread being actively shutting down, and just ignore
      // the join value it fetches.
      if (zx::result join = ThreadJoin(thread); join.is_error()) [[unlikely]] {
        if (join.error_value() != ZX_ERR_INVALID_ARGS) [[unlikely]] {
          CRASH_WITH_UNIQUE_BACKTRACE();
        }
        return join.take_error();
      }
      return zx::ok();

    case Thread::Lifecycle::DONE:
      // It already died before it knew to deallocate itself.
      break;

    default:
      CRASH_WITH_UNIQUE_BACKTRACE();
  }

  // Now the Thread object can be removed from the list of all threads.
  AllThreads().erase(thread);

  // Move the ThreadStorage out of the Thread itself; it's inside that storage.
  // Then immediately let the storage object die, freeing the thread block.
  auto storage = ThreadStorage::FromThread(thread, true);
  storage.AssertLive();

  return zx::ok();
}

}  // namespace LIBC_NAMESPACE_DECL
