// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_INTERNAL_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_INTERNAL_H_

// Intermediate expansion steps for FXT_INTERNAL_EXPAND.
#define FXT_INTERNAL_EXPAND0(...) __VA_ARGS__
#define FXT_INTERNAL_EXPAND1(...) \
  FXT_INTERNAL_EXPAND0(FXT_INTERNAL_EXPAND0(FXT_INTERNAL_EXPAND0(__VA_ARGS__)))
#define FXT_INTERNAL_EXPAND2(...) \
  FXT_INTERNAL_EXPAND1(FXT_INTERNAL_EXPAND1(FXT_INTERNAL_EXPAND1(__VA_ARGS__)))
#define FXT_INTERNAL_EXPAND3(...) \
  FXT_INTERNAL_EXPAND2(FXT_INTERNAL_EXPAND2(FXT_INTERNAL_EXPAND2(__VA_ARGS__)))
#define FXT_INTERNAL_EXPAND4(...) \
  FXT_INTERNAL_EXPAND3(FXT_INTERNAL_EXPAND3(FXT_INTERNAL_EXPAND3(__VA_ARGS__)))

// FXT_INTERNAL_EXPAND(...) expands the given macro expression by evaluating it many times.
#define FXT_INTERNAL_EXPAND(...) \
  FXT_INTERNAL_EXPAND4(FXT_INTERNAL_EXPAND4(FXT_INTERNAL_EXPAND4(__VA_ARGS__)))

// Utility definitions.
#define FXT_INTERNAL_NONE
#define FXT_INTERNAL_COMMA ,

// Intermediate conditional expansion steps for FXT_INTERNAL_NEXT.
#define FXT_INTERNAL_END(...)
#define FXT_INTERNAL_AT_END_A(...) FXT_INTERNAL_AT_END_B
#define FXT_INTERNAL_AT_END_B() 0, FXT_INTERNAL_END
#define FXT_INTERNAL_AT_END(...) FXT_INTERNAL_AT_END_A
#define FXT_INTERNAL_NEXT_A(check, next) FXT_INTERNAL_NEXT_B(check, next, 0)
#define FXT_INTERNAL_NEXT_B(check, next, ...) next FXT_INTERNAL_NONE

// FXT_INTERNAL_NEXT(check, next) conditionally expands to next or FXT_INTERNAL_END to terminate
// expansion by swallowing the parenthesized expression following this macro. Expansion terminates
// when check is the sentinel value ()()().
//
// Example expansion sequence of FXT_INTERNAL_MAP_START with non-sentinel cursor:
// 1. _MAP_START(macro, 0, arg, ()()(), ()()(), ()()(), 0)
// 2. _NEXT_A(_AT_END arg, _MAP_NEXT_A)(macro, arg, ()()(), ()()(), ()()(), 0)
// 3. _NEXT_B(_AT_END arg, _MAP_NEXT_A, 0)(macro, arg, ()()(), ()()(), ()()(), 0)
// 4. _MAP_NEXT_A _NONE(macro, arg, ()()(), ()()(), ()()(), 0)
// 5. macro(arg) _MAP_NEXT_B (macro, ()()(), ()()(), ()()(), 0)
//
// The last step leads to the same terminal case illustrated in the following example.
//
// Example expansion sequence of FXT_INTERNAL_MAP_START with sentinel cursor:
// 1. _MAP_START(macro, 0, ()()(), ()()(), ()()(), 0)
// 2. _NEXT_A(0, _END, _MAP_NEXT_A)(macro, ()()(), ()()(), ()()(), 0)
// 3. _NEXT_B(0, _END, _MAP_NEXT_A, 0)(macro, ()()(), ()()(), ()()(), 0)
// 4. _END _NONE(macro, ()()(), ()()(), ()()(), 0)
// 5. _END(macro, ()()(), ()()(), ()()(), 0)
//
#define FXT_INTERNAL_NEXT(check, next) FXT_INTERNAL_NEXT_A(FXT_INTERNAL_AT_END check, next)

// Intermediate expansion steps for FXT_INTERNAL_MAP.
//
// Input arguments should be in the form: (macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0)
#define FXT_INTERNAL_MAP_START(macro, zero, cursor, ...) \
  FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_NEXT_A)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_NEXT_A(macro, value, cursor, ...) \
  macro(value) FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_NEXT_B)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_NEXT_B(macro, value, cursor, ...) \
  macro(value) FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_NEXT_A)(macro, cursor, __VA_ARGS__)

// Intermediate expansion steps for FXT_INTERNAL_MAP_ARGS.
//
// Input arguments should be in the form: (macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0)
#define FXT_INTERNAL_MAP_ARGS_START(macro, zero, cursor, ...) \
  FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_ARGS_NEXT_A)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_ARGS_NEXT_A(macro, value, cursor, ...) \
  macro value FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_ARGS_NEXT_B)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_ARGS_NEXT_B(macro, value, cursor, ...) \
  macro value FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_ARGS_NEXT_A)(macro, cursor, __VA_ARGS__)

// Intermediate expansion steps for FXT_INTERNAL_MAP_LIST.
//
// Input arguments should be in the form: (macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0)
#define FXT_INTERNAL_MAP_LIST_START(macro, zero, cursor, ...) \
  FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_FIRST)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_FIRST(macro, value, cursor, ...) \
  macro(value) FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_A)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_A(macro, value, cursor, ...) \
  FXT_INTERNAL_COMMA macro(value)                          \
      FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_B)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_B(macro, value, cursor, ...) \
  FXT_INTERNAL_COMMA macro(value)                          \
      FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_A)(macro, cursor, __VA_ARGS__)

// Intermediate expansion steps for FXT_INTERNAL_MAP_LIST_ARGS.
//
// Input arguments should be in the form: (macro, 0, ##__VA_ARGS__, ()()(), ()()(), ()()(), 0)
#define FXT_INTERNAL_MAP_LIST_ARGS_START(macro, zero, cursor, ...) \
  FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_ARGS_FIRST)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_ARGS_FIRST(macro, value, cursor, ...) \
  macro value FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_ARGS_A)(macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_ARGS_A(macro, value, cursor, ...)                           \
  FXT_INTERNAL_COMMA macro value FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_ARGS_B)( \
      macro, cursor, __VA_ARGS__)
#define FXT_INTERNAL_MAP_LIST_ARGS_B(macro, value, cursor, ...)                           \
  FXT_INTERNAL_COMMA macro value FXT_INTERNAL_NEXT(cursor, FXT_INTERNAL_MAP_LIST_ARGS_A)( \
      macro, cursor, __VA_ARGS__)

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_MAP_MACRO_INTERNAL_H_
