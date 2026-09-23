// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_THREAD_LIST_H_
#define LIB_C_THREADS_THREAD_LIST_H_

#include <zircon/compiler.h>

#include <concepts>
#include <functional>
#include <iterator>
#include <mutex>
#include <ranges>
#include <type_traits>

#include "../asm-linkage.h"
#include "../dlfcn/dlfcn-abi.h"
#include "../weak.h"
#include "mutex.h"
#include "src/__support/macros/config.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {

using Thread = ::__pthread;

// This lock guards specifically the list of "all" threads: that is, both the
// gAllThreads variable itself; and also the `next` and `prev` members of every
// Thread reachable from it.  A thread being on this list means it's considered
// "live" for most purposes.  That can include threads that haven't actually
// been started (or even fully created) yet; and threads that have exited or
// are exiting but haven't been joined.  This lock is always held as briefly as
// possible, only while manipulating the _list itself_.  Any thread exit, join,
// or detach needs this lock, not only something costlier like thread creation.
// These must be accessed from ThreadExitFinish(), which is in a basic_abi
// hermetic_source_set(); so they need asm linkage names.
extern Mutex gAllThreadsLock LIBC_ASM_LINKAGE_DECLARE(gAllThreadsLock) __LOCAL;
extern Thread* gAllThreads LIBC_ASM_LINKAGE_DECLARE(gAllThreads) __LOCAL
    __TA_GUARDED(gAllThreadsLock);

// This lock is used to exclude, and synchronize with, changes to the dynamic
// linker data structures consulted by ThreadStorage::GetTlsLayout() and
// ThreadStorage::InitializeTls().  The behavior of these methods must not
// change from the time that ThreadStorage uses them both (one before doing the
// costly allocation VM syscalls and one after), until the new Thread is
// entered onto the gAllThreads list.  Thereafter, the dlopen code paths that
// change their results must safely retrofit every thread on that list while
// holding this lock.  As thread creation is usually both more common and more
// performance-sensitive than dlopen, this uses a reader-writer lock with
// thread creation as "reader" and dlopen (affecting TLS layout) as "writer".
//
// TODO(https://fxbug.dev/342469121): This is only needed in this form while
// using the legacy musl dynamic linker.  The new libdl's implementation of
// dlopen needs some analogous locking, but it can be done at finer grain.
__TA_ACQUIRED_AFTER(kDlfcnLock)        //
__TA_ACQUIRED_BEFORE(gAllThreadsLock)  //
inline constexpr WeakLock<__thread_allocation_inhibit, __thread_allocation_release> kStaticTlsLock;

template <typename IncrementFunction, typename ValueType>
concept Incrementer =
    std::regular_invocable<IncrementFunction, ValueType> &&
    std::convertible_to<std::invoke_result_t<IncrementFunction, ValueType>, ValueType>;

// This just wraps T so that its operator++ std::invoke's Increment as T(T).
// The * and -> operators are passed through for a pointer type.
// Instantiations satisfy std::incrementable.
template <typename T, Incrementer<T> auto Increment>
struct Incrementable {
  using difference_type = std::incrementable_traits<T>::difference_type;
  using value_type = std::remove_cvref_t<std::remove_pointer_t<T>>;

  constexpr bool operator==(const Incrementable&) const = default;
  constexpr auto operator<=>(const Incrementable&) const = default;

  constexpr Incrementable& operator++() {  // prefix
    value = std::invoke(Increment, value);
    return *this;
  }

  constexpr Incrementable operator++(int) {  // postfix
    Incrementable result = *this;
    value = std::invoke(Increment, value);
    return result;
  }

  constexpr auto* operator->() const
    requires(std::is_pointer_v<T>)
  {
    return value;
  }

  constexpr auto* operator->()
    requires(std::is_pointer_v<T>)
  {
    return value;
  }

  constexpr auto& operator*() const
    requires(std::is_pointer_v<T>)
  {
    return *value;
  }

  constexpr auto& operator*()
    requires(std::is_pointer_v<T>)
  {
    return *value;
  }

  T value{};
};

using IncrementableThread = Incrementable<Thread*, &Thread::next>;
static_assert(std::weakly_incrementable<IncrementableThread>);

using ThreadList = std::ranges::iota_view<IncrementableThread, IncrementableThread>;

__TA_REQUIRES(gAllThreadsLock) inline ThreadList AllThreadsLocked() {
  return ThreadList{IncrementableThread{gAllThreads}};
}

class __TA_SCOPED_CAPABILITY AllThreads : public ThreadList {
 public:
  AllThreads() __TA_ACQUIRE(gAllThreadsLock) : ThreadList{IncrementableThread{gAllThreads}} {}

  ~AllThreads() __TA_RELEASE() = default;

  Thread* find(Thread* tcb) const __TA_REQUIRES(gAllThreadsLock) {
    auto it = std::ranges::find(*this, IncrementableThread{tcb});
    return it == end() ? nullptr : (*it).value;
  }

  Thread* FindTp(uintptr_t tp) const __TA_REQUIRES(gAllThreadsLock) {
    return FindTp(reinterpret_cast<void*>(tp));
  }

  Thread* FindTp(void* tp) const __TA_REQUIRES(gAllThreadsLock) {
    // In a race with a freshly-created thread setting up its thread
    // pointer, it might still be zero.
    return tp ? find(tp_to_pthread(tp)) : nullptr;
  }

  void push_front(Thread& tcb) __TA_REQUIRES(gAllThreadsLock) {
    tcb.prevp = &gAllThreads;
    tcb.next = gAllThreads;
    if (tcb.next) {
      tcb.next->prevp = &tcb.next;
    }
    gAllThreads = &tcb;
  }

  void erase(Thread& tcb) __TA_REQUIRES(gAllThreadsLock) {
    *tcb.prevp = tcb.next;
    if (tcb.next) {
      tcb.next->prevp = tcb.prevp;
    }
    // Don't leave old dangling pointers in the soon-to-be-dead Thread.
    tcb.next = nullptr;
    tcb.prevp = nullptr;
  }

 private:
  std::lock_guard<Mutex> lock_{gAllThreadsLock};
};

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_THREAD_LIST_H_
