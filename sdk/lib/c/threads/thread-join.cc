// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/unique-backtrace.h>
#include <lib/zx/result.h>
#include <zircon/syscalls.h>

#include <tuple>

#include "thread-list.h"
#include "thread-storage.h"
#include "thread.h"

namespace LIBC_NAMESPACE_DECL {

zx::result<intptr_t> ThreadJoin(Thread& thread) {
  auto wait = [&thread](Thread::Lifecycle old_state) {
    do {
      switch (_zx_futex_wait(thread.LifecycleFutex(), static_cast<int>(old_state),
                             ZX_HANDLE_INVALID, ZX_TIME_INFINITE)) {
        case ZX_ERR_BAD_STATE:  // Never blocked because it had changed.
        case ZX_OK:             // Woke up because it might have changed.
          old_state = thread.lifecycle_.load(std::memory_order_acquire);
          break;
        default:
          CRASH_WITH_UNIQUE_BACKTRACE();
      }

      // Wait until we reach the DONE state, even if we observe the
      // intermediate EXITING state.
    } while (old_state == Thread::Lifecycle::JOINED || old_state == Thread::Lifecycle::EXITING);

    if (old_state != Thread::Lifecycle::DONE)
      CRASH_WITH_UNIQUE_BACKTRACE();
  };

  // Try to claim the join slot on this thread.
  if (auto old_state = thread.JoinOrDetachLifecycle(Thread::Lifecycle::JOINED); !old_state) {
    wait(Thread::Lifecycle::JOINED);
  } else {
    switch (*old_state) {
      case Thread::Lifecycle::JOINED:
      case Thread::Lifecycle::DETACHED:
        return zx::error{ZX_ERR_INVALID_ARGS};

      case Thread::Lifecycle::EXITING:
        // Since it is undefined to call ThreadJoin on a thread that has
        // already been detached or joined, we assume the state prior to
        // EXITING was JOINABLE, and act as if we had successfully transitioned
        // to JOINED.
        wait(Thread::Lifecycle::EXITING);
        [[fallthrough]];

      case Thread::Lifecycle::DONE:
        break;

      default:
        CRASH_WITH_UNIQUE_BACKTRACE();
    }
  }

  // Take the handle and synchronize with readers.  Then close the handle.
  if (zx::thread handle = thread.TakeHandle(Thread::Lifecycle::DONE); !handle) {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }

  // Now the Thread object can be removed from the list of all threads.
  AllThreads().erase(thread);

  // Extract the return value passed to ThreadExit().
  intptr_t value = std::exchange(thread.join_value, 0);

  // Move the ThreadStorage out of the Thread itself; it's inside that storage.
  // Then immediately let the storage object die, freeing the thread block.
  std::ignore = ThreadStorage::FromThread(thread, true);

  return zx::ok(value);
}

}  // namespace LIBC_NAMESPACE_DECL
