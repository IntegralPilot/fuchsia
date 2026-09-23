// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/object-constants.h>

#include <kernel/ffi.h>
#include <kernel/owned_wait_queue.h>

static_assert(sizeof(OwnedWaitQueue) == kOwnedWaitQueueSize, "OwnedWaitQueue size mismatch");
static_assert(alignof(OwnedWaitQueue) == kOwnedWaitQueueAlign, "OwnedWaitQueue alignment mismatch");

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_owned_wait_queue_init(ffi::Uninitialized<OwnedWaitQueue>* wait_queue) {
  wait_queue->Initialize();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_owned_wait_queue_destroy(OwnedWaitQueue* wait_queue) {
  wait_queue->~OwnedWaitQueue();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_owned_wait_queue_reset_owner_if_no_waiters(OwnedWaitQueue* wait_queue) {
  wait_queue->ResetOwnerIfNoWaiters();
}

}  // extern "C"
