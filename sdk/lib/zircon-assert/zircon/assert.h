// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZIRCON_ASSERT_ZIRCON_ASSERT_H_
#define LIB_ZIRCON_ASSERT_ZIRCON_ASSERT_H_

// For a description of which asserts are enabled at which debug levels, see the documentation for
// GN build argument |assert_level|.

#ifdef _KERNEL
#include <assert.h>
#define ZX_PANIC(args...) PANIC(args)
#define ZX_ASSERT(args...) ASSERT(args)
#define ZX_ASSERT_MSG(args...) ASSERT_MSG(args)
#define ZX_DEBUG_ASSERT(args...) DEBUG_ASSERT(args)
#define ZX_DEBUG_ASSERT_MSG(args...) DEBUG_ASSERT_MSG(args)
#define ZX_DEBUG_ASSERT_COND(args...) DEBUG_ASSERT_COND(args)
#define ZX_DEBUG_ASSERT_MSG_COND(args...) DEBUG_ASSERT_MSG_COND(args)
#define ZX_DEBUG_ASSERT_IMPLEMENTED DEBUG_ASSERT_IMPLEMENTED

#else  // #ifdef _KERNEL

#include <zircon/compiler.h>

__BEGIN_CDECLS
// NOLINTNEXTLINE(bugprone-reserved-identifier)
void __zx_panic(const char* format, ...) __NO_RETURN __PRINTFLIKE(1, 2);
__END_CDECLS

#define ZX_PANIC(fmt, ...) __zx_panic((fmt), ##__VA_ARGS__)

// Conditionally implement ZX_DEBUG_ASSERT based on ZX_ASSERT_LEVEL.
//
// ZX_DEBUG_ASSERT_IMPLEMENTED is intended to be used to conditionalize code
// that is logically part of a debug assert. It's useful for performing complex
// consistency checks that are difficult to work into a ZX_DEBUG_ASSERT
// statement.
#ifdef ZX_ASSERT_LEVEL
#define ZX_DEBUG_ASSERT_IMPLEMENTED (ZX_ASSERT_LEVEL > 1)
#else
#define ZX_DEBUG_ASSERT_IMPLEMENTED 0
#endif

// Notes about the C++ and C versions of the assert macros.
//
// The C++ versions of these macros follow a very specific form for testing the
// assert predicate `x`.  Specifically, the test of the predicate is _always_
// written simply as `if (x)`.  Not:
//
// 1) `if (!x)`                                        // or
// 2) `if ((x))`                                       // or
// 3) `if (unlikely(!(x)))                             // or
// 4) `if (ZX_DEBUG_ASSERT_ENABLED && unlikely(!(x)))  // or, anything else.
//
// It is *always* just `if (x)`.  There is a method to this madness.  The
// primary reason for this is to catch mistakes of the following form:
//
// ZX_DEBUG_ASSERT(my_bool_variable = false);
// ZX_ASSERT(my_int_variable = 5);
//
// Both of these are examples of a pattern where a code author meant to use
// comparison (`==`), but accidentally used assignment (`=`), meaning that the
// first of these examples is always going to fire, while the second will never
// fire.  Both of them will have a side effect of mutating their variable, but a
// ZX_DEBUG_ASSERT's mutation will drop out of a release build.
//
// :: Accidental Assignment Protection ::
//
// This is all very bad, and _should_ be caught by the -Wparentheses warning,
// when enabled (which it usually is). Unfortunately, surrounding an assignment
// operation in a set of `()` will suppress the warning.  Pretty much any other
// form of the predicate test ends up requiring that we add in an extra pair of
// `()`.  This also includes wrapping the predicate in things like `unlikely(x)`
// and `likely(x)`.  So, we have to restrict ourselves strictly to testing with
// `if (x)` in order to get the accidental assignment protection we desire.
//
// :: Temporary Variable Latching ::
//
// A surprise advantage to taking this approach is that it allows us (in C++) to
// more easily write predicates which involve latching to a temporary variable.
// Consider a case where we want to debug assert something about a value
// returned by a function which is expensive to call.  We could write:
//
// DEBUG_ASSERT_MSG(Expensive() == 5,
//                  "Expensive() returned a non-five value (%u)",
//                  Expensive());
//
// But now we are calling the expensive function twice.  We could also say:
//
// [[maybe_unused]] const uint32_t e = Expensive();
// ZX_DEBUG_ASSERT_MSG(e, "Expensive() returned a non-five value (%u)", e);
//
// but it takes a couple of lines, we have to add in a [[maybe_unused]]
// annotation, and we may still be forced to evaluate Expensive if the compiler
// cannot tell that it is guaranteed to have no side effects.
//
// With the new only-`if (x)` form of testing the predicate, however, we can do
// better.  Now, we can write:
//
// ZX_DEBUG_ASSERT_MSG(const uint32_t e = Expensive(); e == 5,
//                     "Expensive() returned a non-five value (%u)", e);
//
// We still get our protection against accidental assignment, we are guaranteed
// to evaluate Expensive exactly once in a debug build, and zero times in a
// release build.
//
// :: WARNING - C code does not get these benefits ::
//
// To write in this style, but also preserve the likely/unlikely hinting
// benefits, we are forced to use C++'s standardized attributes `[[likely]]` and
// `[[unlikely]]``.  These are not available in C, which has to use the old
// compiler attribute macros, which always end up introducing `()`, and
// suppressing the accidental assignment protection.  Right now, there is no
// good way around this, and as long as we are building C code using this
// header, we will need to maintain a version of these macros which do not offer
// the same level of protection.
//
#ifdef __cplusplus

// Only use the [[likely]] attribute if we are sure the compiler is going to
// understand it.
#if defined(__has_cpp_attribute) && (__has_cpp_attribute(likely) >= 201803L)
// NOLINTNEXTLINE(bugprone-reserved-identifier)
#define _ZIRCON_ASSERT_ATTRIBUTE_LIKELY [[likely]]
#else
// NOLINTNEXTLINE(bugprone-reserved-identifier)
#define _ZIRCON_ASSERT_ATTRIBUTE_LIKELY
#endif

// Assert that |x| is true, else panic.
//
// ZX_ASSERT is always enabled and |x| will be evaluated regardless of any build arguments.
#define ZX_ASSERT(x)                                                    \
  do {                                                                  \
    if (x)                                                              \
      _ZIRCON_ASSERT_ATTRIBUTE_LIKELY { break; }                        \
    else { /* NOLINT(readability-else-after-return) */                  \
      ZX_PANIC("ASSERT FAILED at (%s:%d): %s", __FILE__, __LINE__, #x); \
    }                                                                   \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// ZX_ASSERT_MSG is always enabled and |x| will be evaluated regardless of any build arguments.
#define ZX_ASSERT_MSG(x, msg, msgargs...)                                              \
  do {                                                                                 \
    if (x)                                                                             \
      _ZIRCON_ASSERT_ATTRIBUTE_LIKELY { break; }                                       \
    else { /* NOLINT(readability-else-after-return) */                                 \
      ZX_PANIC("ASSERT FAILED at (%s:%d): %s" msg, __FILE__, __LINE__, #x, ##msgargs); \
    }                                                                                  \
  } while (0)

// Assert that |x| is true, else panic.
//
// Depending on build arguments, ZX_DEBUG_ASSERT may or may not be enabled. When disabled, |x| will
// not be evaluated.
#define ZX_DEBUG_ASSERT(x)                                                      \
  do {                                                                          \
    if (ZX_DEBUG_ASSERT_IMPLEMENTED) {                                          \
      if (x)                                                                    \
        _ZIRCON_ASSERT_ATTRIBUTE_LIKELY { break; }                              \
      else { /* NOLINT(readability-else-after-return) */                        \
        ZX_PANIC("DEBUG ASSERT FAILED at (%s:%d): %s", __FILE__, __LINE__, #x); \
      }                                                                         \
    }                                                                           \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// Depending on build arguments, ZX_DEBUG_ASSERT_MSG may or may not be enabled. When disabled, |x|
// will not be evaluated.
#define ZX_DEBUG_ASSERT_MSG(x, msg, msgargs...)                                                \
  do {                                                                                         \
    if (ZX_DEBUG_ASSERT_IMPLEMENTED) {                                                         \
      if (x)                                                                                   \
        _ZIRCON_ASSERT_ATTRIBUTE_LIKELY { break; }                                             \
      else { /* NOLINT(readability-else-after-return) */                                       \
        ZX_PANIC("DEBUG ASSERT FAILED at (%s:%d): %s" msg, __FILE__, __LINE__, #x, ##msgargs); \
      }                                                                                        \
    }                                                                                          \
  } while (0)

#else  // #ifdef __cplusplus

// Assert that |x| is true, else panic.
//
// ZX_ASSERT is always enabled and |x| will be evaluated regardless of any build arguments.
#define ZX_ASSERT(x)                                                    \
  do {                                                                  \
    if (unlikely(!(x))) {                                               \
      ZX_PANIC("ASSERT FAILED at (%s:%d): %s", __FILE__, __LINE__, #x); \
    }                                                                   \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// ZX_ASSERT_MSG is always enabled and |x| will be evaluated regardless of any build arguments.
#define ZX_ASSERT_MSG(x, msg, msgargs...)                                              \
  do {                                                                                 \
    if (unlikely(!(x))) {                                                              \
      ZX_PANIC("ASSERT FAILED at (%s:%d): %s" msg, __FILE__, __LINE__, #x, ##msgargs); \
    }                                                                                  \
  } while (0)

// Assert that |x| is true, else panic.
//
// Depending on build arguments, ZX_DEBUG_ASSERT may or may not be enabled. When disabled, |x| will
// not be evaluated.
#define ZX_DEBUG_ASSERT(x)                                                    \
  do {                                                                        \
    if (ZX_DEBUG_ASSERT_IMPLEMENTED && unlikely(!(x))) {                      \
      ZX_PANIC("DEBUG ASSERT FAILED at (%s:%d): %s", __FILE__, __LINE__, #x); \
    }                                                                         \
  } while (0)

// Assert that |x| is true, else panic with the given message.
//
// Depending on build arguments, ZX_DEBUG_ASSERT_MSG may or may not be enabled. When disabled, |x|
// will not be evaluated.
#define ZX_DEBUG_ASSERT_MSG(x, msg, msgargs...)                                              \
  do {                                                                                       \
    if (ZX_DEBUG_ASSERT_IMPLEMENTED && unlikely(!(x))) {                                     \
      ZX_PANIC("DEBUG ASSERT FAILED at (%s:%d): %s" msg, __FILE__, __LINE__, #x, ##msgargs); \
    }                                                                                        \
  } while (0)

#endif  // #ifdef __cplusplus
        //
// implement _COND versions of ZX_DEBUG_ASSERT which only emit the body if
// ZX_DEBUG_ASSERT_IMPLEMENTED is set
#if ZX_DEBUG_ASSERT_IMPLEMENTED
#define ZX_DEBUG_ASSERT_COND(x) ZX_DEBUG_ASSERT(x)
#define ZX_DEBUG_ASSERT_MSG_COND(x, msg, msgargs...) ZX_DEBUG_ASSERT_MSG(x, msg, msgargs)
#else
#define ZX_DEBUG_ASSERT_COND(x) \
  do {                          \
  } while (0)
#define ZX_DEBUG_ASSERT_MSG_COND(x, msg, msgargs...) \
  do {                                               \
  } while (0)
#endif

#endif  // #ifdef _KERNEL
#endif  // LIB_ZIRCON_ASSERT_ZIRCON_ASSERT_H_
