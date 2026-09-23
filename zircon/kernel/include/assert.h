// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_ASSERT_H_
#define ZIRCON_KERNEL_INCLUDE_ASSERT_H_

// For a description of which asserts are enabled at which debug levels, see the documentation for
// GN build argument |assert_level|.

#include <debug.h>
#include <zircon/compiler.h>

#define PANIC(args...) panic(args)

// Conditionally implement DEBUG_ASSERT based on LK_DEBUGLEVEL in kernel space.
#ifdef LK_DEBUGLEVEL

// DEBUG_ASSERT_IMPLEMENTED is intended to be used to conditionalize code that is logically part of
// a debug assert. It's useful for performing complex consistency checks that are difficult to work
// into a DEBUG_ASSERT statement.
#define DEBUG_ASSERT_IMPLEMENTED (LK_DEBUGLEVEL > 1)
#else
#define DEBUG_ASSERT_IMPLEMENTED 0
#endif

// See comments in //zircon/system/public/zircon/assert.h for details about the
// differences between the C and C++ versions of these ASSERT macros.
//
//
#ifdef __cplusplus

// Only use the [[likely]/[[unlikely]]] attribute if we are sure the compiler is
// going to understand it.
#if defined(__has_cpp_attribute) && (__has_cpp_attribute(likely) >= 201803L)
#define _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY [[likely]]
#else
#define _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY
#endif

// Assert that |x| is true, else panic.
//
// ASSERT is always enabled and |x| will be evaluated regardless of any build arguments.
#define ASSERT(x)                               \
  do {                                          \
    if (x)                                      \
      _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY {} \
    else {                                      \
      assert_fail(__FILE__, __LINE__, #x);      \
    }                                           \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// ASSERT_MSG is always enabled and |x| will be evaluated regardless of any build arguments.
#define ASSERT_MSG(x, msg, msgargs...)                         \
  do {                                                         \
    if (x)                                                     \
      _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY {}                \
    else {                                                     \
      assert_fail_msg(__FILE__, __LINE__, #x, msg, ##msgargs); \
    }                                                          \
  } while (0)

// Assert that |x| is true, else panic.
//
// Depending on build arguments, DEBUG_ASSERT may or may not be enabled. When disabled, |x| will not
// be evaluated.
#define DEBUG_ASSERT(x)                           \
  do {                                            \
    if constexpr (DEBUG_ASSERT_IMPLEMENTED) {     \
      if (x)                                      \
        _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY {} \
      else {                                      \
        assert_fail(__FILE__, __LINE__, #x);      \
      }                                           \
    }                                             \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// Depending on build arguments, DEBUG_ASSERT_MSG may or may not be enabled. When disabled, |x| will
// not be evaluated.
#define DEBUG_ASSERT_MSG(x, msg, msgargs...)                     \
  do {                                                           \
    if constexpr (DEBUG_ASSERT_IMPLEMENTED) {                    \
      if (x)                                                     \
        _ZIRCON_KERNEL_ASSERT_ATTRIBUTE_LIKELY {}                \
      else {                                                     \
        assert_fail_msg(__FILE__, __LINE__, #x, msg, ##msgargs); \
      }                                                          \
    }                                                            \
  } while (0)

#else  // __cplusplus

#define ASSERT(x)                          \
  do {                                     \
    if (unlikely(!(x))) {                  \
      assert_fail(__FILE__, __LINE__, #x); \
    }                                      \
  } while (0)

#define ASSERT_MSG(x, msg, msgargs...)                         \
  do {                                                         \
    if (unlikely(!(x))) {                                      \
      assert_fail_msg(__FILE__, __LINE__, #x, msg, ##msgargs); \
    }                                                          \
  } while (0)

#define DEBUG_ASSERT(x)                               \
  do {                                                \
    if (DEBUG_ASSERT_IMPLEMENTED && unlikely(!(x))) { \
      assert_fail(__FILE__, __LINE__, #x);            \
    }                                                 \
  } while (0)

#define DEBUG_ASSERT_MSG(x, msg, msgargs...)                   \
  do {                                                         \
    if (DEBUG_ASSERT_IMPLEMENTED && unlikely(!(x))) {          \
      assert_fail_msg(__FILE__, __LINE__, #x, msg, ##msgargs); \
    }                                                          \
  } while (0)

// make sure static_assert() is defined, even in C
#if !defined(static_assert)
#define static_assert(e, msg) _Static_assert(e, msg)
#endif

#endif  // __cplusplus

// implement _COND versions of DEBUG_ASSERT which only emit the body if
// DEBUG_ASSERT_IMPLEMENTED is set
#if DEBUG_ASSERT_IMPLEMENTED
#define DEBUG_ASSERT_COND(x) DEBUG_ASSERT(x)
#define DEBUG_ASSERT_MSG_COND(x, msg, msgargs...) DEBUG_ASSERT_MSG(x, msg, msgargs)
#else
#define DEBUG_ASSERT_COND(x) \
  do {                       \
  } while (0)
#define DEBUG_ASSERT_MSG_COND(x, msg, msgargs...) \
  do {                                            \
  } while (0)
#endif

// Use DEBUG_ASSERT or ASSERT instead.
//
// assert() is defined only because third-party code used in the kernel expects it.
#define assert(x) DEBUG_ASSERT(x)

__BEGIN_CDECLS

// The following functions are called when an assert fails.
void assert_fail(const char *file, int line, const char *expression) __NO_RETURN __NO_INLINE;
void assert_fail_msg(const char *file, int line, const char *expression, const char *fmt,
                     ...) __NO_RETURN __NO_INLINE __PRINTFLIKE(4, 5);

__END_CDECLS

#endif  // ZIRCON_KERNEL_INCLUDE_ASSERT_H_
