// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FBL_ALLOC_CHECKER_H_
#define FBL_ALLOC_CHECKER_H_

#include <stddef.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <concepts>
#include <memory>
#include <new>
#include <type_traits>
#include <utility>

namespace fbl {

// An object which is passed to operator new to allow client code to handle
// allocation failures.  Once armed by operator new, the client must call `check()`
// to verify the state of the allocation checker before it goes out of scope.
//
// Use it like this:
//
//     AllocChecker ac;
//     MyObject* obj = new (&ac) MyObject();
//     if (!ac.check()) {
//         // handle allocation failure (obj will be null)
//     }
class AllocChecker {
 public:
  AllocChecker() = default;
  ~AllocChecker() {
    if (ZX_DEBUG_ASSERT_IMPLEMENTED && unlikely(armed_)) {
      CheckNotCalledPanic();
    }
  }

  // Arm the AllocChecker. Once armed, `check` must be called prior to destruction.
  void arm(size_t size, bool result) {
    if (ZX_DEBUG_ASSERT_IMPLEMENTED && unlikely(armed_)) {
      ArmedTwicePanic();
    }
    armed_ = true;
    ok_ = (size == 0 || result);
  }

  // Allow calls with anything explicitly convertible to bool, not just
  // anything implicitly convertible to bool.  This makes smart-pointer types
  // usable directly as the second argument.
  void arm(size_t size, const auto& result) { arm(size, static_cast<bool>(result)); }

  // Return true if the previous allocation succeeded.
  bool check() {
    armed_ = false;
    return likely(ok_);
  }

 private:
  // If called, abort program execution with an error.
  [[noreturn]] static void CheckNotCalledPanic();
  [[noreturn]] static void ArmedTwicePanic();

  bool armed_ = false;
  bool ok_ = false;
};

namespace internal {

inline void* checked(size_t size, AllocChecker& ac, void* mem) {
  ac.arm(size, mem != nullptr);
  return mem;
}

}  // namespace internal

// A version of std::make_unique that accepts an AllocChecker as its first argument.
template <typename T, typename... Args>
  requires(!std::is_array_v<T>)
std::unique_ptr<T> make_unique_checked(AllocChecker& ac, Args&&... args) {
  return std::unique_ptr<T>(new (ac) T(std::forward<Args>(args)...));
}

template <typename T, typename... Args>
  requires(!std::is_array_v<T>)
std::unique_ptr<T> make_unique_checked(AllocChecker* ac, Args&&... args) {
  return make_unique_checked<T>(*ac, std::forward<Args>(args)...);
}

template <typename T, typename... Args>
  requires(std::is_unbounded_array_v<T>)
std::unique_ptr<T> make_unique_checked(AllocChecker& ac, size_t size) {
  static_assert(std::is_constructible_v<std::remove_extent_t<T>>);
  return std::unique_ptr<T>(new (ac) std::remove_extent_t<T>[size]());
}

template <std::constructible_from<> T, typename... Args>
  requires(std::is_unbounded_array_v<T>)
std::unique_ptr<T> make_unique_checked(AllocChecker* ac, size_t size) {
  return make_unique_checked<T>(*ac, size);
}

// Likewise for std::make_unique_for_overwrite.
template <std::default_initializable T>
  requires(!std::is_array_v<T>)
std::unique_ptr<T> make_unique_for_overwrite_checked(AllocChecker& ac) {
  return std::unique_ptr<T>(new (ac) T);
}

template <typename T>
  requires(std::is_unbounded_array_v<T> && std::default_initializable<std::remove_extent_t<T>>)
std::unique_ptr<T> make_unique_for_overwrite_checked(AllocChecker& ac, size_t size) {
  return std::unique_ptr<T>(new (ac) std::remove_extent_t<T>[size]);
}

}  // namespace fbl

// Versions of the C++ `new` operator that accept an AllocChecker.
//
// These can be used as follows:
//
//    fbl::AllocChecker ac;
//    Object* foo = new (ac) Object();
//    if (!ac.check()) {
//      // failed.
//    }
//    // ...
//
// We use different implementations in userspace and kernel:
//
//   * The kernel intentionally omits any operator new or new[] signatures
//     that _aren't_ the fbl::AllocChecker ones so no unchecked allocations
//     can get into the kernel accidentally (stopped by link-time failure).
//     It provides its own implementations that don't need to be inlined.
//
//   * Userspace uses the standard C++ library std::nothrow_t versions of the
//     `new` operator.  These work better with sanitizers such as ASAN that
//     ensure that the correct `new`/`new[]` operator is paired with the
//     correct `delete`/`delete[]` operator.
//
#if _KERNEL
void* operator new(size_t size, fbl::AllocChecker& ac) noexcept;
void* operator new(size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept;
void* operator new[](size_t size, fbl::AllocChecker& ac) noexcept;
void* operator new[](size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept;
#else
inline void* operator new(size_t size, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, operator new(size, std::nothrow_t()));
}
inline void* operator new(size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, operator new(size, align, std::nothrow_t()));
}
inline void* operator new[](size_t size, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, operator new[](size, std::nothrow_t()));
}
inline void* operator new[](size_t size, std::align_val_t align, fbl::AllocChecker& ac) noexcept {
  return fbl::internal::checked(size, ac, operator new[](size, align, std::nothrow_t()));
}
#endif  // !_KERNEL

// For compatibility with older uses, ac can either be passed by reference (ac)
// or by explicit pointer (&ac).
inline void* operator new(size_t size, fbl::AllocChecker* ac) noexcept {
  return operator new(size, *ac);
}
inline void* operator new(size_t size, std::align_val_t align, fbl::AllocChecker* ac) noexcept {
  return operator new(size, align, *ac);
}
inline void* operator new[](size_t size, fbl::AllocChecker* ac) noexcept {
  return operator new[](size, *ac);
}
inline void* operator new[](size_t size, std::align_val_t align, fbl::AllocChecker* ac) noexcept {
  return operator new[](size, align, *ac);
}

#endif  // FBL_ALLOC_CHECKER_H_
