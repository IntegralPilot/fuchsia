// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <atomic>

#include "libc.h"
#include "src/stdlib/exit.h"
#include "thread.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

// This is replaced with a real definition when dlerror() code is linked in.
[[gnu::weak]] void ThreadDlfcnCleanup() {}

// This does the final "normal" work on the exiting thread: running
// destructors, etc.  This is reached either directly from a call to
// thrd_exit() or pthread_exit(), or from a thread function returning.
[[noreturn]] void ThreadExit(intptr_t value) {
  __tls_run_dtors();         // Run C++ thread_local destructors.
  __thread_tsd_run_dtors();  // Run tss_create / pthread_key_create destructors.

  // It's impossible to determine whether this is "the last thread" until
  // performing the atomic decrement, since multiple threads could exit at the
  // same time.  If it was the last thread, then the whole process exits.
  if (__libc.thread_count.fetch_sub(1) == 1) {
    // The old value was one, so now the value is zero: this is the last thread
    // and should call exit(0).  But first, put the thread count back to one,
    // "undoing" the thread exit to return to being a normal single-threaded
    // process while executing the process exit.  The atexit handlers could do
    // anything, including starting new threads or even reentering here after
    // new threads might be waiting to join this one!
    __libc.thread_count.store(1);
    exit(0);
  }

  // Finally, no more user code will run on this thread and call back into
  // libc, e.g. dlerror().  Clean up any allocation stored for dlerror().
  ThreadDlfcnCleanup();

  // Store the value for ThreadJoin() to find.  Any joiners already waiting
  // will be woken via futex last thing in ThreadExitFinish().
  Thread& self = *__pthread_self();
  self.join_value = value;

  // After this point the sanitizer runtime will tear down its state, so we
  // cannot run any more sanitized code.  The rest is done in code compiled for
  // the basic machine ABI.
  ThreadExitFinish(self);
}

}  // namespace LIBC_NAMESPACE_DECL
