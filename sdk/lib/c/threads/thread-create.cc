// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/zx/result.h>
#include <pthread.h>
#include <zircon/sanitizer.h>

#include "../weak.h"
#include "thread-list.h"
#include "thread-storage.h"
#include "thread.h"
#include "threads_impl.h"

extern "C" decltype(__sanitizer_before_thread_create_hook) __sanitizer_before_thread_create_hook
    [[gnu::weak]];

namespace LIBC_NAMESPACE_DECL {
namespace {

using SanitizerBeforeThreadCreateHook = Weak<__sanitizer_before_thread_create_hook>;

// TODO(https://fxbug.dev/342469121): This is only needed in this form while
// using the legacy musl dynamic linker.  The new libdl's implementation of
// dlopen needs some analogous locking, but it can be done at finer grain.

constinit pthread_rwlock_t gThreadCreationLock = PTHREAD_RWLOCK_INITIALIZER;

// Many threads could be reading the TLS state.  They don't exclude each other
// from doing separate ThreadStorage::Allocate() calls.
__TA_EXCLUDES(kStaticTlsLock) void LockForThreadCreate() {
  pthread_rwlock_rdlock(&gThreadCreationLock);
}
__TA_EXCLUDES(kStaticTlsLock) void UnlockForThreadCreate() {
  pthread_rwlock_unlock(&gThreadCreationLock);
}
constexpr WeakLock<LockForThreadCreate, UnlockForThreadCreate> kLockForThreadCreate;

}  // namespace

using ::__thread_allocation_inhibit, ::__thread_allocation_release;

// dlopen calls this under another lock.  Only one dlopen call can be modifying
// state at a time.  It excludes all ThreadStorage::Allocate() calls until the
// corresponding Thread goes on the gAllThreads list (thread-list.h).
extern "C" void __thread_allocation_inhibit() {
  pthread_rwlock_wrlock(&LIBC_NAMESPACE::gThreadCreationLock);
}

extern "C" void __thread_allocation_release() {
  pthread_rwlock_unlock(&LIBC_NAMESPACE::gThreadCreationLock);
}

zx::result<CreatedThread> ThreadCreate(ThreadAttributes attrs) {
  assert(!attrs.name.empty());
  std::string_view thread_name = attrs.name.str();
  std::string_view vmo_name = thread_name;

  Thread& self = *__pthread_self();

  // First allocate the storage for the Thread, its stacks, etc.
  CreatedThread thread;
  {
    // TODO(https://fxbug.dev/342469121): With the legacy musl dynamic linker,
    // the "static" TLS size can change dynamically with dlopen calls.  The
    // GetTlsLayout() and InitializeTls() calls inside Allocate() need to be
    // "atomic" with respect to adding the thread to the global list, from the
    // perspective of dlopen.  Either this thread used the current TLS sizes
    // after the last dlopen change, or the next dlopen will see this thread on
    // its list to be updated for new TLS sizes.
    std::lock_guard static_tls_lock{kLockForThreadCreate};
    ThreadStorage storage;
    zx::result allocate = storage.Allocate(self.create_handles, vmo_name, attrs.stack, attrs.guard);
    if (allocate.is_error()) {
      return allocate.take_error();
    }

    // Take ownership of the Thread here, moving storage ownership into it.
    thread.reset(*allocate);
    std::move(storage).ToThread(*thread);

    // With that ownership goes ownership of its place on the all-threads list.
    AllThreads().push_front(*thread);
  }

  // Hereafter, when this CreatedThread object dies, that will remove it from
  // the all-threads list and reclaim all the storage.  But there is still no
  // kernel thread yet, so it's time to create that now.
  zx::unowned_process process{self.create_handles.process};
  zx::thread thread_handle;
  zx_status_t status = zx::thread::create(
      *process, thread_name.data(), static_cast<uint32_t>(thread_name.size()), 0, &thread_handle);
  if (status != ZX_OK) [[unlikely]] {
    return zx::error{status};
  }
  thread->handle_ = thread_handle.release();

  const Thread::Lifecycle initial_state =  // State must be set before ThreadStart.
      attrs.detached ? Thread::Lifecycle::DETACHED : Thread::Lifecycle::JOINABLE;
  // The state update is always ordered after the handle update.
  thread->lifecycle_.store(initial_state, std::memory_order_release);

  // This is the same in every thread, with the initial thread's slot holding
  // the original source of truth rather than any global location.
  thread->abi.stack_guard = self.abi.stack_guard;

  // This is inherited from the creating thread, but might be changed with
  // thrd_set_zx_create_handles() or thrd_set_zx_process().
  thread->create_handles = self.create_handles;

  // The user callback supplies the void* given to the next user callback.
  const std::span stack = ThreadStorage::ThreadMachineStack(*thread);
  thread->sanitizer_hook = SanitizerBeforeThreadCreateHook::Or<void*>{}(
      ToC11Thread(*thread), attrs.detached, attrs.name.c_str(), stack.data(), stack.size_bytes());

  return zx::ok(std::move(thread));
}

}  // namespace LIBC_NAMESPACE_DECL
