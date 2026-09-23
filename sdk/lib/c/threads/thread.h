// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_THREAD_H_
#define LIB_C_THREADS_THREAD_H_

#include <lib/zx/result.h>
#include <pthread.h>
#include <threads.h>
#include <zircon/sanitizer.h>

#include <cerrno>
#include <concepts>
#include <cstdint>
#include <memory>

#include "../asm-linkage.h"
#include "../startup/start-main.h"
#include "../zircon/vmar.h"
#include "../zircon/zx-name.h"

struct __pthread;  // NOLINT(bugprone-reserved-identifier): "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

// The legacy code defines `struct __pthread` in "threads_impl.h".
using Thread = ::__pthread;

struct ThreadAttributes {
  // ThreadCreate demands a nonempty name.  Callers can use these to set one.

  constexpr ThreadAttributes WithDefaultName(const ZxName& default_name) const {
    ThreadAttributes result = *this;
    if (name.empty()) {
      result.name = default_name;
    }
    return result;
  }

  constexpr ThreadAttributes WithDefaultName(const char* fmt, auto... args) const {
    ThreadAttributes result = *this;
    if (name.empty()) {
      result.name = ZxName::Printf(fmt, args...);
    }
    return result;
  }

  PageRoundedSize stack = InitialStackSize();       // Default to main thread's size.
  PageRoundedSize guard = PageRoundedSize::Page();  // Default to a one-page guard.
  bool detached = false;                            // If set, start detached.
  ZxName name;                                      // Optional.
};

// This has the same actual ABI as both int(void*), as used in C11
// thrd_create(); and void*(void*), as used in POSIX pthread_create().
using ThreadFunction = intptr_t(void*) [[clang::cfi_unchecked_callee]];
inline ThreadFunction* ToThreadFunction(int (*func)(void*)) {
  return reinterpret_cast<ThreadFunction*>(reinterpret_cast<uintptr_t>(func));
}
inline ThreadFunction* ToThreadFunction(void* (*func)(void*)) {
  return reinterpret_cast<ThreadFunction*>(reinterpret_cast<uintptr_t>(func));
}

// The C11 <threads.h> thrd_t is actually just the Thread*.
inline thrd_t ToC11Thread(Thread& thread) { return reinterpret_cast<thrd_t>(&thread); }
inline Thread* FromC11Thread(thrd_t thread) { return reinterpret_cast<Thread*>(thread); }

// The POSIX <pthread.h> pthread_t is the same thing too.
inline pthread_t ToPthread(Thread& thread) { return reinterpret_cast<pthread_t>(&thread); }
inline Thread* FromPthread(pthread_t thread) { return reinterpret_cast<Thread*>(thread); }

// The CreatedThread object owns a Thread object and the kernel thread created
// with it by ThreadCreate, along with its place on the gAllThreads list.  All
// those get cleaned up together if the CreatedThread dies before it's consumed
// by a successful ThreadStart, transferring ownership to the running thread.
struct CreatedThreadDeleter {
  void operator()(Thread*) const;
};
using CreatedThread = std::unique_ptr<Thread, CreatedThreadDeleter>;

// Create a new Thread.  This does all the allocation and creates the kernel
// thread.  The new Thread object is initialized, owns that zx::thread handle,
// and is attached to the global thread list.  The thread is not running yet and
// will be destroyed when the CreatedThread object dies before ThreadStart.
// Kernel operations can now be done via the thread handle to affect the thread
// (set scheduling parameters, etc.) before it starts running.
zx::result<CreatedThread> ThreadCreate(ThreadAttributes attrs);

// After a new Thread has been fully created, this actually starts it running.
// The new thread will call ThreadExit(func(arg)).  Once the thread is running,
// it owns its own storage and kernel handle, so the CreatedThread is released.
// But unless the thread is detached, the caller now owns it via the Thread*
// until that is passed to ThreadJoin (or detached).
zx::result<Thread*> ThreadStart(CreatedThread thread, ThreadFunction* func, void* arg);

// Combines ThreadCreate and ThreadStart.  This is templatized with converter
// functions for the public thread and error types (ToC11Thread / ToPthread,
// C11ThreadError / PthreadError) rather than just being a non-template
// function returning zx::result<Thread*> because thrd_create is required to
// write its result parameter before the new thread might read it back out of
// that same memory; the caller unpacking the result would be too late.
template <std::invocable<Thread&> auto NewThread, std::invocable<zx_status_t> auto Status>
decltype(Status(std::declval<zx_status_t>())) ThreadCreateAndStart(
    decltype(NewThread(std::declval<Thread&>()))* new_thread, ThreadAttributes attrs,
    ThreadFunction* func, void* arg) {
  using NewThreadType = decltype(NewThread(std::declval<Thread&>()));
  zx::result created = ThreadCreate(attrs);
  if (created.is_error()) [[unlikely]] {
    return Status(created.error_value());
  }
  // The result parameter must be set before the new thread starts running.
  // It's valid to use memory that the new thread will itself read from!
  *new_thread = NewThread(**created);
  zx::result started = ThreadStart(*std::move(created), func, arg);
  if (started.is_error()) [[unlikely]] {
    // The stale value was already written, but should never be used.  It's
    // undefined what value this gets in the error case, but it's a bad idea to
    // leak what's effectively a known-stale pointer under any circumstances.
    // In most cases, the result parameter just wouldn't be touched at all
    // until all the error cases have been ruled out.  But that's not possible
    // since ThreadStart might fail though it must be after storing the result.
    // Saving and restoring the old value instead of clearing it would just
    // give the false impression of stability, when in fact it's just another
    // new race.  So always clobber the result parameter, leaving only the race
    // when no new thread actually existed but the stale value was visible
    // there briefly in memory _on this thread_ and _maybe_ on others.
    *new_thread = NewThreadType{};
    return Status(started.error_value());
  }
  return Status(ZX_OK);
}

// This underlies thrd_exit() and pthread_exit(); they differ only in value
// type.  It calls thread-local destructors and so forth, and finally calls
// ThreadExitFinish to do the real tear-down work.
[[noreturn]] void ThreadExit(intptr_t value);

// This is called by ThreadExit to destroy per-thrad <dlfcn.h> (dlerror) state.
void ThreadDlfcnCleanup();

// The last phase of exit is compiled using only the basic machine ABI so it
// can do some stack-switching and then free all the main thread stacks.
[[noreturn]] void ThreadExitFinish(Thread& self)  //
    LIBC_ASM_LINKAGE_DECLARE(ThreadExitFinish);

// This underlies thrd_join() and pthread_join().  It yields the value passed
// to ThreadExit().
zx::result<intptr_t> ThreadJoin(Thread& thread);

// This underlies thrd_detach() and pthread_detach().  As soon as it returns
// success, the Thread reference is no longer safe to use in any way because
// the thread can exit and free the storage itself at any time.
zx::result<> ThreadDetach(Thread& thread);

// Make the callback on the thread's TLS segments.
void OnTlsSegments(Thread& thread, sanitizer_memory_snapshot_callback_t* callback,
                   void* callback_arg);

// Convert Zircon error to C11 <threads.h> return value.
constexpr int C11ThreadError(zx_status_t status) {
  switch (status) {
    case ZX_OK:
      return thrd_success;
    case ZX_ERR_NO_MEMORY:
    case ZX_ERR_NO_RESOURCES:
      return thrd_nomem;
    case ZX_ERR_TIMED_OUT:
      return thrd_timedout;
    default:
      return thrd_error;
  }
}

// Convert Zircon error to POSIX errno.
constexpr int PthreadError(zx_status_t status) {
  switch (status) {
    case ZX_OK:
      return 0;
    case ZX_ERR_INVALID_ARGS:
      return EINVAL;
    case ZX_ERR_ACCESS_DENIED:
      return EPERM;
    case ZX_ERR_TIMED_OUT:
      return ETIMEDOUT;
    case ZX_ERR_NO_MEMORY:
    case ZX_ERR_NO_RESOURCES:
      return EAGAIN;
    case ZX_ERR_NOT_FOUND:
      // These two are possible in pthread_create due to thrd_set_zx_process.
    case ZX_ERR_BAD_HANDLE:
    case ZX_ERR_WRONG_TYPE:
      return ESRCH;
    default:
      __builtin_abort();
  }
}

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_THREAD_H_
