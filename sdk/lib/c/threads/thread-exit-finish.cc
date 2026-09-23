// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/zircon-internal/unique-backtrace.h>
#include <zircon/sanitizer.h>

#include <atomic>
#include <cassert>
#include <optional>

#include "../weak.h"
#include "thread-list.h"
#include "thread-storage.h"
#include "thread.h"
#include "threads_impl.h"

// This file is compiled in the basic machine ABI (user.basic) environment.
// It switches off of, and then frees, the thread's main stacks.

extern "C" decltype(__sanitizer_thread_exit_hook) __sanitizer_thread_exit_hook [[gnu::weak]];

namespace LIBC_NAMESPACE_DECL {
namespace {

using SanitizerExitHook = Weak<__sanitizer_thread_exit_hook>;

constexpr uintptr_t kStackAlignment = elfldltl::AbiTraits<>::kStackAlignment<>;

[[maybe_unused]] constexpr size_t kMinimumStack = 256;

// This is the very last thing to run on an exiting thread.  Not only does it
// use only the basic machine ABI, but it runs on a tiny stub stack that's
// scavenged from the unused part of the thread block.
[[noreturn]] void FinalExit() {
  Thread& self = *__pthread_self();

  // Transfer ownership of the thread stacks, but not the thread block itself.
  // This will one day just be `old_storage = std::move(self.storage_);`.
  std::optional<ThreadStorage> old_storage = ThreadStorage::FromThread(self, false);

  // The TCB fields that __sanitizer_memory_snapshot reads for the stacks were
  // cleared by FromThread (and would be by the eventual ThreadStorage move
  // ctor used directly from a Thread member).  Make sure those stores have
  // definitely completed before the stacks are deallocated.  In case we get
  // suspended by __sanitizer_memory_snapshot, the TCB is always expected to
  // contain valid pointers.
  std::atomic_signal_fence(std::memory_order_seq_cst);

  // Now actually free the stacks.
  old_storage.reset();

  // The same VMAR handle used to map the thread block will be used to unmap
  // it.  It's owned by the user (or is just the zx::vmar::root_self() handle)
  // and must have been kept alive at least for the lifetime of this thread.
  // If the handle is no longer valid, we'll crash instead of a detached exit.
  zx::unowned_vmar unmap_vmar{self.storage_handles.thread_block_vmar};

  // Enter EXITING state, and see what sort of cleanup should happen based on
  // the old state.  This deallocates the TCB region too for the detached case.
  // If not detached, ThreadJoin will deallocate it.  This always makes the
  // thread-list removal callback before deallocating the TCB.  Hence
  // __sanitizer_memory_snapshot should not consider the thread to be "alive"
  // any more, safely before the memory might be unmapped.
  const Thread::Lifecycle old_lifecycle =
      self.lifecycle_.exchange(Thread::Lifecycle::EXITING, std::memory_order_release);
  switch (old_lifecycle) {
    case Thread::Lifecycle::DETACHED: {
      AllThreads().erase(self);
      zx::thread handle = self.TakeHandle(Thread::Lifecycle::EXITING);
      // The address and size stored for the thread block include the full VMAR
      // around it; the unmap implicitly destroys that VMAR.
      _zx_vmar_unmap_handle_close_thread_exit(unmap_vmar->get(), self.storage_thread_block_address,
                                              self.storage_thread_block_size, handle.release());
      CRASH_WITH_UNIQUE_BACKTRACE();
      break;
    }

    case Thread::Lifecycle::JOINABLE:
      // Nobody's watching right now, but they might start watching as we
      // exit.  Just in case, behave as if we've been joined and wake the
      // futex on our way out.
      [[fallthrough]];

    case Thread::Lifecycle::JOINED:
      // Somebody loves us!  Or at least intends to inherit when we die.  Wake
      // the _zx_futex_wait in zxr_thread_join, and then die.  This has to be
      // done with the special four-in-one vDSO call because as soon as the
      // state transitions to DONE, the joiner is free to unmap our stack out
      // from under us.  Note there is a benign race here still: if the address
      // is unmapped and our futex_wake fails, it's OK; if the memory is reused
      // for something else and our futex_wake tickles somebody completely
      // unrelated, well, that's why any zx_futex_wait can always have spurious
      // wakeups.
      _zx_futex_wake_handle_close_thread_exit(
          self.LifecycleFutex(), 1, static_cast<int>(Thread::Lifecycle::DONE), ZX_HANDLE_INVALID);
      CRASH_WITH_UNIQUE_BACKTRACE();
      break;

    case Thread::Lifecycle::DONE:
    case Thread::Lifecycle::EXITING:
    case Thread::Lifecycle::FREED:
      // Cannot be in DONE, EXITING, or FREED and reach here.
      CRASH_WITH_UNIQUE_BACKTRACE();
      break;
  }

  // Some bogus value different from any in the enum would be clobberation,
  // whereas DONE, EXITING, or FREED would more likely be a bug in this code.
  CRASH_WITH_UNIQUE_BACKTRACE();
}

// Switch to a new machine stack and call FinalExit(), which cannot return.
[[noreturn]] void FinalExitOnStack(uintptr_t sp) {
  __asm__ volatile(
#if defined(__aarch64__)
      R"""(
        mov sp, %[sp]
        bl %cc[FinalExit]
      )"""
#elif defined(__riscv)
      R"""(
        mv sp, %[sp]
        call %cc[FinalExit]
      )"""
#elif defined(__x86_64__)
      R"""(
        mov %[sp], %%rsp
        call %cc[FinalExit]
      )"""
#else
#error "unsupported machine"
#endif
      :
      : [sp] "r"(sp), [FinalExit] "X"(FinalExit));
  __builtin_trap();
}

// Compute a new machine stack pointer to start at for the call to FinalExit().
// This can use some part of the thread block that won't overlap with the
// actual Thread object that stays valid until a join is complete.
uintptr_t FinalExitSp(Thread& self) {
  const std::span thread_block = ThreadStorage::ThreadThreadBlock(self);

  if constexpr (elfldltl::TlsTraits<>::kTlsNegative) {
    // The thread descriptor is at the end of the region, so the space
    // before it (formerly TLS) is available as the temporary stack.
    const uintptr_t sp = reinterpret_cast<uintptr_t>(&self) & -kStackAlignment;
    [[maybe_unused]] const uintptr_t thread_block_base =
        reinterpret_cast<uintptr_t>(thread_block.data());
    assert(sp > thread_block_base);
    assert(sp - thread_block_base >= kMinimumStack);
    return sp;
  } else {
    // The thread descriptor is at the start of the region, so the rest of
    // the space up to the guard page is available as the temporary stack.
    const uintptr_t sp = reinterpret_cast<uintptr_t>(  //
        thread_block.data() + thread_block.size());
    assert(sp % kStackAlignment == 0);
    assert(thread_block.size_bytes() >= sizeof(Thread) + kMinimumStack);
    return sp;
  }
}

}  // namespace

[[noreturn]] void ThreadExitFinish(Thread& self) {
  // Notify the sanitizer runtime that the thread is about to exit.  As soon as
  // the exit returns, no more sanitizer runtime callbacks are safe to make.
  // Hence this is called in a function that won't itself ever be instrumented.
  SanitizerExitHook::Call(self.sanitizer_hook, ToC11Thread(self));

  // Switch to the temporary stack and free the real stacks on the way out.
  FinalExitOnStack(FinalExitSp(self));
}

}  // namespace LIBC_NAMESPACE_DECL
