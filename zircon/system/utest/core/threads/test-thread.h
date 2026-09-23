// Copyright 2025 The Fuchsia Authors. All rights reserved.
//
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_THREADS_TEST_THREAD_H_
#define ZIRCON_SYSTEM_UTEST_CORE_THREADS_TEST_THREAD_H_

#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>

#include <concepts>
#include <string_view>

// This handles launching a thread to run a thread-functions.h function.  That
// code is all compiled specially to use only the basic machine ABI, and each
// function simply takes one scalar argument and calls zx_thread_exit() so it
// never returns.  This allows very simple thread launching with just a small
// stack and no other setup or libc-like threads runtime.  The thread lifetime
// is only handled by the TestThread object and the kernel thread API itself.
// When a TestThread is destroyed, the stack and thread are fully cleaned up.

class TestThread {
 public:
  static inline const size_t kDefaultGuardSize = zx_system_get_page_size();
  static inline const size_t kDefaultStackSize = zx_system_get_page_size();

  TestThread() = default;
  TestThread(TestThread&&) = default;
  TestThread& operator=(TestThread&&) = default;

  // The destructor does Wait() if necessary to be sure the thread is done
  // running before destruction unmaps its stack.
  ~TestThread();

  // Create the thread and prepare its stack, with zxtest assertion failures if
  // anything goes wrong.  After ASSERT_NO_FATAL_FAILURE(t.Init(...)),
  // t.thread() is ready to be started but hasn't been yet.
  void Init(std::string_view name,
            // If valid, this is an existing VMO to use (from offset 0) to map
            // the stack from.  Otherwise a fresh VMO is created and its handle
            // not kept anywhere once it's mapped.
            zx::unowned_vmo stack_vmo = {},
            // Where to create the thread.
            zx::unowned_process process = zx::process::self(),
            zx::unowned_vmar = zx::vmar::root_self(),
            // Stack sizes (must be whole page sizes).
            size_t stack_size = kDefaultStackSize, size_t guard_size = kDefaultGuardSize);

  const zx::thread& thread() const { return thread_; }

  // After Init (and any other operations on thread() the test might need),
  // Start can take any of the thread-functions.h functions and its argument.
  // Each is a [[noreturn]] void function of one argument that's either a
  // pointer type or an integral type (e.g. zx_handle_t).  Each ends with a
  // zx_thread_exit() system call, so Wait() can detect when it's finished.

  template <typename T>
  void Start(void (*f)(T*), T* arg) {
    Start(reinterpret_cast<uintptr_t>(f), reinterpret_cast<uintptr_t>(arg));
  }

  template <std::integral T>
  void Start(void (*f)(T), T arg) {
    // Widen to the argument register size as appropriate for signedness.
    using W = std::conditional_t<std::is_signed_v<T>, intptr_t, uintptr_t>;
    Start(reinterpret_cast<uintptr_t>(f), static_cast<W>(arg));
  }

  // Wait until the thread is dead, unless there is no thread.
  void Wait();

 private:
  void Start(uintptr_t entry, uintptr_t arg);

  zx::thread thread_;
  zx::vmar stack_;
  uintptr_t sp_ = 0;
};

#endif  // ZIRCON_SYSTEM_UTEST_CORE_THREADS_TEST_THREAD_H_
